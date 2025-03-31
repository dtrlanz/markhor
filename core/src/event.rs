use std::{
    fmt,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Weak,
    },
};
use crossbeam_skiplist::SkipSet;

/// Represents an event that can be dispatched to listeners.
pub trait Event: fmt::Debug + Send + Sync {
    /// The return type of the event handler.
    type HandlerReturnType: fmt::Debug;

    /// Updates the event with the result of a listener's callback.
    /// This method is called after each listener's callback returns.
    ///
    /// Override this method to modify the event based on the listener's return value.
    /// The default implementation does nothing.
    fn update(&mut self, _handler_result: Self::HandlerReturnType) {}
}

#[derive(Debug)]
struct ListenerEntry<E: Event> {
    // Use Weak pointer to avoid cycles and allow listeners to be dropped automatically.
    callback: Weak<dyn Fn(&E) -> E::HandlerReturnType + Send + Sync>,
    // Use an atomic counter to ensure listeners are called in the order they were added.
    order: usize,
}

// Implement comparison traits based solely on the insertion order.
impl<E: Event> Eq for ListenerEntry<E> {}

impl<E: Event> PartialEq for ListenerEntry<E> {
    fn eq(&self, other: &Self) -> bool {
        self.order == other.order
    }
}

impl<E: Event> Ord for ListenerEntry<E> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.order.cmp(&other.order)
    }
}

impl<E: Event> PartialOrd for ListenerEntry<E> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// Global atomic counter to assign a unique order to each listener upon creation.
static LISTENER_ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A list of listeners for a specific event type `E`.
///
/// Listeners are stored in a `SkipSet` ordered by their insertion sequence,
/// ensuring they are called in the order they were added.
pub struct ListenerList<E: Event> {
    // Using SkipSet for efficient, ordered insertion and iteration.
    inner: SkipSet<ListenerEntry<E>>,
}

impl<E: Event + 'static> ListenerList<E> {
    /// Creates a new, empty listener list.
    pub fn new() -> Self {

        ListenerList {
            inner: SkipSet::new(),
        }
    }

    /// Dispatches an event to all registered listeners in order.
    ///
    /// This method iterates through the listeners. For each listener:
    /// 1. It attempts to upgrade the `Weak` pointer to the callback `Arc`.
    /// 2. If successful, it calls the callback with the current event state.
    /// 3. It calls the event's `update` method with the callback's return value.
    /// 4. If the `Weak` pointer cannot be upgraded (meaning the `Listener` was dropped),
    ///    the stale `ListenerEntry` is removed from the list.
    ///
    /// This method is `pub(crate)` to ensure that only code within the same crate
    /// as this module can dispatch events, maintaining encapsulation for event emitters.
    pub(crate) fn dispatch(&self, event: &mut E) {
        let mut entries_to_remove = Vec::new();

        for entry in self.inner.iter() {
            if let Some(callback_arc) = entry.callback.upgrade() {
                // Callback is still valid, execute it
                let result = callback_arc(event);
                event.update(result);
            } else {
                // Callback `Arc` was dropped, mark this entry for removal
                // We collect orders because removing directly during iteration is problematic
                // and ListenerEntry itself doesn't implement Clone/Copy easily.
                entries_to_remove.push(entry.order);
            }
        }

        let dummy_arc: Arc::<dyn Fn(&E) -> E::HandlerReturnType + Send + Sync> = Arc::new(Self::dummy_handler);
        

        // Remove stale entries outside the main iteration loop
        // Note: Re-fetching the entry by order is needed because SkipSet::remove requires a borrowed value.
        // This is less efficient than direct removal but necessary with the current structure.
        // A potential optimization could involve altering ListenerEntry or using a different removal strategy.
        for order in entries_to_remove {
            // Create a temporary key just for removal lookup
            let key = ListenerEntry {
                callback: Arc::downgrade(&dummy_arc), // Callback content doesn't matter for comparison
                order,
            };
            self.inner.remove(&key);
        }
    }

    // Dummy handler function to satisfy the type requirements of the Weak pointer.
    // This function is never called.
    fn dummy_handler(_: &E) -> E::HandlerReturnType {
        unreachable!()
    }
}

impl<E: Event + 'static> Default for ListenerList<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: Event> fmt::Debug for ListenerList<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Customize Debug output to maybe show the count or a hint of orders
        f.debug_struct("ListenerList")
         .field("listener_count", &self.inner.len())
         .finish()
    }
}

/// Represents an active listener registration.
///
/// When this struct is dropped, the listener is effectively deregistered,
/// and its entry will be cleaned up from the `ListenerList` upon the next dispatch.
/// It holds a strong reference (`Arc`) to the callback closure.
pub struct Listener<E: Event> {
    // Keep the Arc alive for the lifetime of the Listener struct.
    // The ListenerList holds only a Weak reference.
    #[allow(dead_code)] // Arc is kept for lifetime management, not direct use here
    arc: Arc<dyn Fn(&E) -> E::HandlerReturnType + Send + Sync>,
    // Store order to ensure listeners are called in the order they were added.
    order: usize,
}

impl<E: Event + 'static> Listener<E> {
    /// Creates a new listener and registers it with the given `ListenerList`.
    ///
    /// # Arguments
    /// * `listeners`: The `ListenerList` to register with.
    /// * `callback`: The closure to execute when the event is dispatched.
    ///
    /// # Returns
    /// A `Listener` instance. Keep this instance alive for as long as you want the
    /// listener to be active. When it's dropped, the listener becomes inactive.
    pub fn new<F>(listeners: &ListenerList<E>, callback: F) -> Self
    where
        F: Fn(&E) -> E::HandlerReturnType + Send + Sync + 'static,
    {
        let order = LISTENER_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
        let arc: Arc<dyn Fn(&E) -> E::HandlerReturnType + Send + Sync> = Arc::new(callback);
        let entry = ListenerEntry {
            callback: Arc::downgrade(&arc),
            order,
        };
        listeners.inner.insert(entry);

        Listener { arc, order }
    }
}

impl<E: Event> fmt::Debug for Listener<E> {
    /// Formats the `Listener` for debugging.
    /// This includes a number reflecting the order of the listener in the list.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Listener")
            .field("order", &self.order)
            .finish()
    }
}

/// Macro to define a struct holding named `ListenerList` fields for various event types.
///
/// This macro generates a struct with the specified name, containing public fields
/// for each event listener list.
//#[macro_export]
macro_rules! define_event_listeners {
    ($struct_name:ident { $($field_name:ident: $event_type:ty),* $(,)? }) => {
        /// Holds listener lists for various events.
        #[derive(Debug, Default)]
        pub struct $struct_name {
            $(
                pub $field_name: $crate::event::ListenerList<$event_type>,
            )*
        }

        impl $struct_name {
            /// Creates a new instance with empty listener lists.
            pub fn new() -> Self {
                Self {
                    $(
                        $field_name: $crate::event::ListenerList::new(),
                    )*
                }
            }
        }
    };
}

pub(crate) use define_event_listeners;


#[cfg(test)]
mod tests {
    use std::{cell::RefCell, sync::Mutex};

    // Import necessary items from the parent module (containing the event system)
    use super::*;

    // --- Define Sample Event Structs ---
    #[derive(Debug, Clone)]
    pub struct Event1(i32);
    impl Event for Event1 {
        type HandlerReturnType = i32;

        fn update(&mut self, value: i32) {
            self.0 += value;
            println!("Event1 updated: current value = {}", self.0); // Added logging
        }
    }

    #[derive(Debug, Clone)]
    pub struct Event2(i32);
    impl Event for Event2 {
        type HandlerReturnType = i32;
        fn update(&mut self, value: i32) {
            self.0 *= value; // Changed logic for variety
             println!("Event2 updated: current value = {}", self.0); // Added logging
        }
    }

    // --- Use the Macro ---
    // Define a struct `TestEvents` that will hold the listener lists for Event1 and Event2
    define_event_listeners!(TestEvents {
        on_event1: Event1,
        on_event2: Event2,
    });

    #[test]
    fn test_listener_registration_and_dispatch() {
        // Create an instance of the generated struct
        let listeners = TestEvents::new();

        // Register listeners using Listener::new
        let _listener1_handle = Listener::new(&listeners.on_event1, |event| {
            println!("Listener 1 for Event1 called with event: {:?}", event);
            10 // Return value for Event1::update
        });
        let _listener2_handle = Listener::new(&listeners.on_event1, |event| {
             println!("Listener 2 for Event1 called with event: {:?}", event);
            5 // Return value for Event1::update
        });
        let _listener3_handle = Listener::new(&listeners.on_event2, |event| {
             println!("Listener 1 for Event2 called with event: {:?}", event);
            3 // Return value for Event2::update
        });

        // --- Dispatch Event1 ---
        let mut event1 = Event1(100); // Initial value
        println!("Dispatching Event1...");
        // Call dispatch directly on the ListenerList (possible because tests are in the same crate)
        listeners.on_event1.dispatch(&mut event1);
        // Expected: 100 (initial) + 10 (listener1) + 5 (listener2) = 115
        assert_eq!(event1.0, 115);
        println!("Event1 dispatch complete. Final value: {}", event1.0);

        // --- Dispatch Event2 ---
        let mut event2 = Event2(5); // Initial value
        println!("Dispatching Event2...");
        listeners.on_event2.dispatch(&mut event2);
        // Expected: 5 (initial) * 3 (listener3) = 15
        assert_eq!(event2.0, 15);
        println!("Event2 dispatch complete. Final value: {}", event2.0);
    }

    #[test]
    fn test_listener_cleanup_on_drop() {
        let listeners = TestEvents::new();
        let listener_count_before: usize;
        let listener_count_after_drop: usize;
        let listener_count_after_dispatch: usize;

        {
            // Create a listener in a limited scope
            let _listener_temp = Listener::new(&listeners.on_event1, |_| 1);
            listener_count_before = listeners.on_event1.inner.len();
            println!("Listener count before drop: {}", listener_count_before);
            assert_eq!(listener_count_before, 1);
            // _listener_temp is dropped here
        }

        // The Weak ref still exists, so the count might not change immediately
        // (implementation detail of SkipSet iterators/cleanup)
        listener_count_after_drop = listeners.on_event1.inner.len();
         println!("Listener count after drop (before dispatch): {}", listener_count_after_drop);
         // Asserting it might still be 1 is valid, as cleanup happens during dispatch
         assert_eq!(listener_count_after_drop, 1);

        // Dispatch an event to trigger cleanup
        let mut event1 = Event1(0);
        println!("Dispatching event to trigger cleanup...");
        listeners.on_event1.dispatch(&mut event1);

        // Now the dropped listener should have been removed
        listener_count_after_dispatch = listeners.on_event1.inner.len();
        println!("Listener count after dispatch (should be 0): {}", listener_count_after_dispatch);
        assert_eq!(listener_count_after_dispatch, 0);

         // Ensure the event wasn't updated (since the listener was gone)
        assert_eq!(event1.0, 0);
    }

     #[test]
    fn test_listener_order() {
        let listeners = TestEvents::new();
        let call_order = Arc::new(Mutex::new(Vec::new()));
        let (co_a, co_b, co_c) = (call_order.clone(), call_order.clone(), call_order.clone());

        let _listener_a = Listener::new(&listeners.on_event1, move |_| {
            println!("Listener A called");
            co_a.try_lock().unwrap().push("A");
            1 // Return value doesn't matter for this test
        });
        let _listener_b = Listener::new(&listeners.on_event1, move |_| {
            println!("Listener B called");
            co_b.try_lock().unwrap().push("B");
            1
        });
         let _listener_c = Listener::new(&listeners.on_event1, move |_| {
             println!("Listener C called");
             co_c.try_lock().unwrap().push("C");
            1
        });


        let mut event1 = Event1(0);
        println!("Dispatching event to test order...");
        listeners.on_event1.dispatch(&mut event1);

        println!("Call order: {:?}", call_order);
        assert_eq!(std::mem::take(&mut *call_order.try_lock().unwrap()), vec!["A", "B", "C"]);
    }
}