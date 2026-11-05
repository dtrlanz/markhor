use std::sync::{Arc, Mutex, Weak};

use smallvec::SmallVec;

type DynListener<T> = Arc<dyn Listener<T>>;

// Ids are assigned in increasing order and appended, so `callbacks` stays sorted by id.
// Most events have 0 or 1 listeners, so one entry is stored inline without a heap allocation.
struct Listeners<T> {
    next_id: u64,
    listeners: SmallVec<[(u64, DynListener<T>); 1]>,
}

impl<T> Listeners<T> {
    fn remove(&mut self, id: u64) -> Option<DynListener<T>> {
        let pos = self.listeners.binary_search_by_key(&id, |(i, _)| *i).ok()?;
        Some(self.listeners.remove(pos).1)
    }
}

trait Unsubscribe: Send + Sync {
    fn unsubscribe(&self, id: u64);
}

impl<T: 'static> Unsubscribe for Mutex<Listeners<T>> {
    fn unsubscribe(&self, id: u64) {
        // Drop the removed listener after the lock is released, in case it owns a Subscription.
        let _removed = self.lock().unwrap().remove(id);
    }
}

pub struct Event<T> {
    listeners: Arc<Mutex<Listeners<T>>>,
}

impl<T: 'static> Default for Event<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: 'static> Event<T> {
    pub fn new() -> Self {
        Self {
            listeners: Arc::new(Mutex::new(Listeners {
                next_id: 0,
                listeners: SmallVec::new(),
            })),
        }
    }

    pub fn add_listener(&self, listener: impl Listener<T> + 'static) -> Subscription {
        let listener: DynListener<T> = Arc::new(listener);
        let id = {
            let mut listeners = self.listeners.lock().unwrap();
            let id = listeners.next_id;
            listeners.next_id += 1;
            listeners.listeners.push((id, listener));
            id
        };

        let weak = Arc::downgrade(&self.listeners);
        let weak: Weak<dyn Unsubscribe> = weak;
        Subscription {
            target: Some((weak, id)),
        }
    }

    pub fn dispatch(&self, payload: &T) {
        // Clone the `Arc`s out so the lock is released before any listener runs.
        let snapshot: SmallVec<[DynListener<T>; 2]> = self
            .listeners
            .lock()
            .unwrap()
            .listeners
            .iter()
            .map(|(_, listener)| listener.clone())
            .collect();

        for listener in snapshot {
            listener.invoke(payload);
        }
    }
}

pub struct Subscription {
    target: Option<(Weak<dyn Unsubscribe>, u64)>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some((weak, id)) = self.target.take() {
            if let Some(listeners) = weak.upgrade() {
                listeners.unsubscribe(id);
            }
        }
    }
}

pub trait Listener<T>: Send + Sync {
    fn invoke(&self, payload: &T);
}

// Implements `Listener` for closures taking `&T1, ..., &Tn` for the given tuple type.
macro_rules! impl_listener {
    ($($T:ident),* $(,)?) => {
        impl<F, $($T),*> Listener<($($T,)*)> for F
        where
            F: Fn($(&$T),*) + Send + Sync,
        {
            #[allow(non_snake_case)]
            fn invoke(&self, payload: &($($T,)*)) {
                let ($(ref $T,)*) = *payload;
                self($($T),*);
            }
        }
    };
}

impl_listener!();
impl_listener!(T1);
impl_listener!(T1, T2);
impl_listener!(T1, T2, T3);
impl_listener!(T1, T2, T3, T4);
impl_listener!(T1, T2, T3, T4, T5);
impl_listener!(T1, T2, T3, T4, T5, T6);
impl_listener!(T1, T2, T3, T4, T5, T6, T7);
impl_listener!(T1, T2, T3, T4, T5, T6, T7, T8);
impl_listener!(T1, T2, T3, T4, T5, T6, T7, T8, T9);
impl_listener!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
impl_listener!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
impl_listener!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12);
impl_listener!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13);
impl_listener!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14);
impl_listener!(
    T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15
);
impl_listener!(
    T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16
);

/// Generates a struct whose fields are `Event`s, with a `Default` impl.
///
/// ```ignore
/// events! {
///     pub struct FooEvents {
///         alien_invasion: AlienInfo,
///         zombie_apocalypse: ZombieInfo,
///     }
/// }
/// ```
#[macro_export]
macro_rules! events {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field:ident : $payload:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Default)]
        $vis struct $name {
            $(
                $(#[$field_meta])*
                $vis $field: $crate::event::Event<$payload>,
            )*
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    #[derive(Debug, PartialEq)]
    pub struct AlienInfo {
        pub id: u32,
    }

    #[derive(Debug, PartialEq)]
    pub struct ZombieInfo {
        pub id: u32,
    }

    events! {
        struct FooEvents {
            alien_invasion: (AlienInfo,),
            zombie_apocalypse: (ZombieInfo,),
        }
    }

    struct Foo {
        events: FooEvents,
        alien_invasion_active: bool,
        zombie_apocalypse_active: bool,
    }

    impl Foo {
        fn new() -> Self {
            Self {
                events: FooEvents::default(),
                alien_invasion_active: false,
                zombie_apocalypse_active: false,
            }
        }

        fn on_alien_invasion(
            &mut self,
            callback: impl Fn(&AlienInfo) + Send + Sync + 'static,
        ) -> Subscription {
            self.events.alien_invasion.add_listener(callback)
        }

        fn on_zombie_apocalypse(
            &mut self,
            callback: impl Fn(&ZombieInfo) + Send + Sync + 'static,
        ) -> Subscription {
            self.events.zombie_apocalypse.add_listener(callback)
        }

        fn check_weather(&mut self) {
            if self.alien_invasion_active {
                self.events.alien_invasion.dispatch(&(AlienInfo { id: 1 },));
            }
            if self.zombie_apocalypse_active {
                self.events
                    .zombie_apocalypse
                    .dispatch(&(ZombieInfo { id: 2 },));
            }
        }
    }

    fn counting_alien_listener() -> (
        Arc<AtomicUsize>,
        impl Fn(&AlienInfo) + Send + Sync + 'static,
    ) {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        (count, move |_: &AlienInfo| {
            c.fetch_add(1, Ordering::SeqCst);
        })
    }

    fn assert_send<T: Send>(_: &T) {}

    #[test]
    fn dispatch_passes_payload_to_listener() {
        let event: Event<(AlienInfo,)> = Event::new();
        let received = Arc::new(Mutex::new(None));
        let r = received.clone();
        let _sub = event.add_listener(move |info: &AlienInfo| {
            *r.lock().unwrap() = Some(info.id);
        });

        event.dispatch(&(AlienInfo { id: 42 },));

        assert_eq!(*received.lock().unwrap(), Some(42));
    }

    #[test]
    fn dispatch_calls_listeners_in_registration_order() {
        let event: Event<(AlienInfo,)> = Event::new();
        let order = Arc::new(Mutex::new(Vec::new()));
        let subs: Vec<Subscription> = (1..=3)
            .map(|n| {
                let order = order.clone();
                event.add_listener(move |_: &AlienInfo| order.lock().unwrap().push(n))
            })
            .collect();

        event.dispatch(&(AlienInfo { id: 0 },));

        assert_eq!(*order.lock().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn dispatch_supports_sixteen_payload_fields() {
        type P = (
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
        );
        let event: Event<P> = Event::new();
        let sum = Arc::new(AtomicUsize::new(0));
        let s = sum.clone();
        let _sub = event.add_listener(
            move |a: &u32,
                  b: &u32,
                  c: &u32,
                  d: &u32,
                  e: &u32,
                  f: &u32,
                  g: &u32,
                  h: &u32,
                  i: &u32,
                  j: &u32,
                  k: &u32,
                  l: &u32,
                  m: &u32,
                  n: &u32,
                  o: &u32,
                  p: &u32| {
                let total = [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p]
                    .iter()
                    .map(|x| **x as usize)
                    .sum::<usize>();
                s.store(total, Ordering::SeqCst);
            },
        );

        event.dispatch(&(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16));

        assert_eq!(sum.load(Ordering::SeqCst), 136);
    }

    #[test]
    fn dispatch_with_no_payload_calls_listener() {
        let event: Event<()> = Event::new();
        let (count, callback) = {
            let count = Arc::new(AtomicUsize::new(0));
            let c = count.clone();
            (count, move || {
                c.fetch_add(1, Ordering::SeqCst);
            })
        };
        let _sub = event.add_listener(callback);

        event.dispatch(&());

        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dispatch_with_no_listeners_does_nothing() {
        let event: Event<AlienInfo> = Event::new();

        event.dispatch(&AlienInfo { id: 0 });
    }

    #[test]
    fn dropping_subscription_stops_listener() {
        let event: Event<(AlienInfo,)> = Event::new();
        let (count, callback) = counting_alien_listener();
        let sub = event.add_listener(callback);

        drop(sub);
        event.dispatch(&(AlienInfo { id: 0 },));

        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    // Dropping a subscription whose listener owns another subscription to the same
    // event used to re-lock the (non-reentrant) mutex on the same thread and deadlock.
    #[test]
    fn dropping_subscription_whose_callback_owns_another_does_not_deadlock() {
        let event: Event<(AlienInfo,)> = Event::new();
        let (count, callback) = counting_alien_listener();
        let inner = event.add_listener(callback);
        let holder = Mutex::new(Some(inner));
        let outer = event.add_listener(move |_: &AlienInfo| {
            let _ = &holder;
        });

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(outer);
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("dropping the outer subscription deadlocked");

        event.dispatch(&(AlienInfo { id: 0 },));
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn listener_stays_active_while_subscription_is_held() {
        let event: Event<(AlienInfo,)> = Event::new();
        let (count, callback) = counting_alien_listener();
        let _sub = event.add_listener(callback);

        event.dispatch(&(AlienInfo { id: 0 },));
        event.dispatch(&(AlienInfo { id: 0 },));

        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn events_of_different_types_are_independent() {
        let mut foo = Foo::new();
        let (alien_count, alien_callback) = counting_alien_listener();
        let zombie_count = Arc::new(AtomicUsize::new(0));
        let z = zombie_count.clone();
        let _alien_sub = foo.on_alien_invasion(alien_callback);
        let _zombie_sub = foo.on_zombie_apocalypse(move |_: &ZombieInfo| {
            z.fetch_add(1, Ordering::SeqCst);
        });
        foo.zombie_apocalypse_active = true;

        foo.check_weather();

        assert_eq!(alien_count.load(Ordering::SeqCst), 0);
        assert_eq!(zombie_count.load(Ordering::SeqCst), 1);
    }

    // Dispatch iterates over a snapshot of the listeners taken when it starts. A listener
    // dropped mid-dispatch is still called once in that dispatch, and never again after.
    #[test]
    fn dropping_subscription_during_dispatch_still_runs_it_this_round() {
        let event: Event<(AlienInfo,)> = Event::new();
        let (count, callback) = counting_alien_listener();
        let slot: Arc<Mutex<Option<Subscription>>> = Arc::new(Mutex::new(None));
        let slot_for_first = slot.clone();
        // Listener A drops subscription B (stored in `slot`) while dispatch is running.
        let _first = event.add_listener(move |_: &AlienInfo| {
            slot_for_first.lock().unwrap().take();
        });
        *slot.lock().unwrap() = Some(event.add_listener(callback));

        // B is still in the snapshot, so it runs once here.
        event.dispatch(&(AlienInfo { id: 0 },));
        assert_eq!(count.load(Ordering::SeqCst), 1);

        // B has been dropped, so it no longer runs.
        event.dispatch(&(AlienInfo { id: 0 },));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    // A listener added mid-dispatch is missing from the snapshot, so it first runs on the
    // next dispatch. The `added` flag makes the adding listener register its child only once.
    #[test]
    fn listener_added_during_dispatch_is_not_called_this_round() {
        let event = Arc::new(Event::<(AlienInfo,)>::new());
        let (count, _) = counting_alien_listener();
        let kept: Arc<Mutex<Vec<Subscription>>> = Arc::new(Mutex::new(Vec::new()));
        let added = Arc::new(AtomicBool::new(false));

        let adder_event = event.clone();
        let adder_count = count.clone();
        let adder_kept = kept.clone();
        let adder_added = added.clone();
        // Listener A registers a counting listener C on its first call only.
        let _adder = event.add_listener(move |_: &AlienInfo| {
            if !adder_added.swap(true, Ordering::SeqCst) {
                let c = adder_count.clone();
                let sub = adder_event.add_listener(move |_: &AlienInfo| {
                    c.fetch_add(1, Ordering::SeqCst);
                });
                // Keep C's subscription alive so it stays registered.
                adder_kept.lock().unwrap().push(sub);
            }
        });

        // C was added mid-dispatch, so it is not in this dispatch's snapshot.
        event.dispatch(&(AlienInfo { id: 0 },));
        assert_eq!(count.load(Ordering::SeqCst), 0);

        // C is now registered and runs.
        event.dispatch(&(AlienInfo { id: 0 },));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn subscription_outliving_event_does_not_panic() {
        let _sub = {
            let event: Event<(AlienInfo,)> = Event::new();
            event.add_listener(|_: &AlienInfo| {})
        };
    }

    #[test]
    fn foo_is_send() {
        assert_send(&Foo::new());
    }

    #[test]
    fn check_weather_dispatches_only_active_events() {
        let mut foo = Foo::new();
        let (alien_count, alien_callback) = counting_alien_listener();
        let zombie_count = Arc::new(AtomicUsize::new(0));
        let z = zombie_count.clone();
        let _alien_sub = foo.on_alien_invasion(alien_callback);
        let _zombie_sub = foo.on_zombie_apocalypse(move |_: &ZombieInfo| {
            z.fetch_add(1, Ordering::SeqCst);
        });
        foo.alien_invasion_active = true;

        foo.check_weather();

        assert_eq!(alien_count.load(Ordering::SeqCst), 1);
        assert_eq!(zombie_count.load(Ordering::SeqCst), 0);
    }
}
