use std::{fmt, sync::{atomic::AtomicUsize, Arc, Weak}};
use crossbeam_skiplist::SkipSet;

/// Represents an event that can be dispatched to listeners.
pub trait Event: fmt::Debug + Send + Sync {
    /// The return type of the event handler.
    type HandlerReturnType: fmt::Debug;

    /// Updates the event with the result of a listener's callback.
    fn update(&mut self, _element: Self::HandlerReturnType) {
        // Default implementation does nothing.
        // Override this method to modify the event based on the listener's return value.
    }
}

#[derive(Debug)]
struct ListenerEntry<E: Event> {
    callback: Weak<dyn Fn(&E) -> E::HandlerReturnType + Send + Sync>,
    order: usize,
}

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

static LISTENER_ID: AtomicUsize = AtomicUsize::new(0);

/// A list of listeners for a specific event type.
pub struct ListenerList<E: Event> {
    inner: SkipSet<ListenerEntry<E>>,
}

impl<E: Event + 'static> ListenerList<E> {
    pub fn new() -> Self {
        ListenerList {
            inner: SkipSet::new(),
        }
    }
}

impl<E: Event> fmt::Debug for ListenerList<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.inner.iter()).finish()
    }
}

/// A listener for a specific event type.
pub struct Listener<E: Event> {
    arc: Arc<dyn Fn(&E) -> E::HandlerReturnType + Send>,
}

impl<E: Event + 'static> Listener<E> {
    pub fn new<F: (Fn(&E) -> E::HandlerReturnType) + Send + Sync + 'static>(listeners: &ListenerList<E>, callback: F) -> Self {
        let arc: Arc<dyn Fn(&E) -> E::HandlerReturnType + Send + Sync> = Arc::new(callback);
        listeners.inner.insert(ListenerEntry {
            callback: Arc::downgrade(&arc),
            order: LISTENER_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        });
        Listener { arc }
    }
}

/// Macro to define multiple event types and their corresponding listener lists.
///
/// This macro generates an `EventManager` struct that holds all the defined
/// event types and provides methods to dispatch events to their listeners.
#[macro_export]
macro_rules! events {
    ($($name:ident: $event_type:ty),*) => {
        #[derive(Debug)]
        pub struct EventManager {
            event_types: EventTypes,
        }
        impl EventManager {
            fn new() -> Self {
                EventManager {
                    event_types: EventTypes::new(),
                }
            }
        
            fn dispatch<E: Event + 'static>(listeners: &ListenerList<E>, event: &mut E) {
                // Dispatch event to listeners
                for listener in listeners.inner.iter() {
                    if let Some(callback) = listener.callback.upgrade() {
                        let value = callback(event);
                        event.update(value);
                    } else {
                        // Remove the listener if it has been dropped
                        listeners.inner.remove(&*listener);
                    }
                    
                }
            }
        }

        impl std::ops::Deref for EventManager {
            type Target = EventTypes;
        
            fn deref(&self) -> &Self::Target {
                &self.event_types
            }
        }

        /// A collection of all event types.
        #[derive(Debug)]
        pub struct EventTypes {
            $(
                pub $name: ListenerList<$event_type>,
            )*
        }
        impl EventTypes {
            pub fn new() -> Self {
                EventTypes {
                    $(
                        $name: ListenerList::new(),
                    )*
                }
            }
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    pub struct Event1(i32);
    impl Event for Event1 {
        type HandlerReturnType = i32;
    
        fn update(&mut self, value: i32) {
            self.0 += value;
        }
    }
    
    #[derive(Debug)]
    pub struct Event2(i32);
    impl Event for Event2 {
        type HandlerReturnType = i32;
        fn update(&mut self, value: i32) {
            self.0 += value;
        }
    }



    #[test]
    fn test_manual_event_manager() {

        pub struct EventManager {
            event_types: EventTypes,
        }
        impl EventManager {
            fn new() -> Self {
                EventManager {
                    event_types: EventTypes::new(),
                }
            }
        
            fn dispatch<E: Event + 'static>(listeners: &ListenerList<E>, event: &mut E) {
                // Dispatch event to listeners
                for listener in listeners.inner.iter() {
                    if let Some(callback) = listener.callback.upgrade() {
                        let value = callback(event);
                        event.update(value);
                    } else {
                        // Remove the listener if it has been dropped
                        listeners.inner.remove(&*listener);
                    }
                    
                }
            }
        }
        
        impl std::ops::Deref for EventManager {
            type Target = EventTypes;
        
            fn deref(&self) -> &Self::Target {
                &self.event_types
            }
        }
        
        #[derive(Debug)]
        pub struct EventTypes {
            event1: ListenerList<Event1>,
            event2: ListenerList<Event2>,
        }
        
        impl EventTypes {
            pub fn new() -> Self {
                EventTypes {
                    event1: ListenerList::new(),
                    event2: ListenerList::new(),
                }
            }
        }

        let manager = EventManager::new();
        let _listener1 = Listener::new(&manager.event1, |_| 1);
        let _listener2 = Listener::new(&manager.event2, |_| 2);
        let mut event1 = Event1(0);
        EventManager::dispatch(&manager.event1, &mut event1);
        assert_eq!(event1.0, 1);    // 0 + 1 = 1
        let mut event2 = Event2(0);
        EventManager::dispatch(&manager.event2, &mut event2);
        assert_eq!(event2.0, 2);    // 0 + 2 = 2
    }

    #[test]
    fn test_macro_event_manager() {
        events! {
            event1: Event1,
            event2: Event2
        }

        let manager = EventManager::new();
        let _listener1 = Listener::new(&manager.event1, |_| 1);
        let _listener2 = Listener::new(&manager.event2, |_| 2);
        let mut event1 = Event1(0);
        EventManager::dispatch(&manager.event1, &mut event1);
        assert_eq!(event1.0, 1);    // 0 + 1 = 1
        let mut event2 = Event2(0);
        EventManager::dispatch(&manager.event2, &mut event2);
        assert_eq!(event2.0, 2);    // 0 + 2 = 2
    }

}
