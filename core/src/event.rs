use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};

type Callback<T> = Arc<dyn Fn(&T) + Send + Sync>;

struct Listeners<T> {
    next_id: u64,
    callbacks: BTreeMap<u64, Callback<T>>,
}

pub struct Event<T> {
    listeners: Arc<Mutex<Listeners<T>>>,
}

impl<T: 'static> Event<T> {
    pub fn new() -> Self {
        Self {
            listeners: Arc::new(Mutex::new(Listeners {
                next_id: 0,
                callbacks: BTreeMap::new(),
            })),
        }
    }

    pub fn add_listener(&self, callback: impl Fn(&T) + Send + Sync + 'static) -> Subscription {
        let callback: Callback<T> = Arc::new(callback);
        let id = {
            let mut listeners = self.listeners.lock().unwrap();
            let id = listeners.next_id;
            listeners.next_id += 1;
            listeners.callbacks.insert(id, callback);
            id
        };

        let weak: Weak<Mutex<Listeners<T>>> = Arc::downgrade(&self.listeners);
        Subscription {
            remove: Some(Box::new(move || {
                if let Some(listeners) = weak.upgrade() {
                    listeners.lock().unwrap().callbacks.remove(&id);
                }
            })),
        }
    }

    pub fn dispatch(&self, payload: &T) {
        // Clone the callbacks out so the lock is released before any callback runs.
        let snapshot: Vec<Callback<T>> = self
            .listeners
            .lock()
            .unwrap()
            .callbacks
            .values()
            .cloned()
            .collect();

        for callback in snapshot {
            callback(payload);
        }
    }
}

pub struct Subscription {
    remove: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(remove) = self.remove.take() {
            remove();
        }
    }
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

    struct FooEvents {
        alien_invasion: Event<AlienInfo>,
        zombie_apocolypse: Event<ZombieInfo>,
    }

    impl FooEvents {
        fn new() -> Self {
            Self {
                alien_invasion: Event::new(),
                zombie_apocolypse: Event::new(),
            }
        }
    }

    struct Foo {
        events: FooEvents,
        alien_invasion_active: bool,
        zombie_apocolypse_active: bool,
    }

    impl Foo {
        fn new() -> Self {
            Self {
                events: FooEvents::new(),
                alien_invasion_active: false,
                zombie_apocolypse_active: false,
            }
        }

        fn on_alien_invasion(
            &mut self,
            callback: impl Fn(&AlienInfo) + Send + Sync + 'static,
        ) -> Subscription {
            self.events.alien_invasion.add_listener(callback)
        }

        fn on_zombie_apocolypse(
            &mut self,
            callback: impl Fn(&ZombieInfo) + Send + Sync + 'static,
        ) -> Subscription {
            self.events.zombie_apocolypse.add_listener(callback)
        }

        fn check_weather(&mut self) {
            if self.alien_invasion_active {
                self.events.alien_invasion.dispatch(&AlienInfo { id: 1 });
            }
            if self.zombie_apocolypse_active {
                self.events
                    .zombie_apocolypse
                    .dispatch(&ZombieInfo { id: 2 });
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
        let event: Event<AlienInfo> = Event::new();
        let received = Arc::new(Mutex::new(None));
        let r = received.clone();
        let _sub = event.add_listener(move |info: &AlienInfo| {
            *r.lock().unwrap() = Some(info.id);
        });

        event.dispatch(&AlienInfo { id: 42 });

        assert_eq!(*received.lock().unwrap(), Some(42));
    }

    #[test]
    fn dispatch_calls_listeners_in_registration_order() {
        let event: Event<AlienInfo> = Event::new();
        let order = Arc::new(Mutex::new(Vec::new()));
        let subs: Vec<Subscription> = (1..=3)
            .map(|n| {
                let order = order.clone();
                event.add_listener(move |_: &AlienInfo| order.lock().unwrap().push(n))
            })
            .collect();

        event.dispatch(&AlienInfo { id: 0 });

        assert_eq!(*order.lock().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn dispatch_with_no_listeners_does_nothing() {
        let event: Event<AlienInfo> = Event::new();

        event.dispatch(&AlienInfo { id: 0 });
    }

    #[test]
    fn dropping_subscription_stops_listener() {
        let event: Event<AlienInfo> = Event::new();
        let (count, callback) = counting_alien_listener();
        let sub = event.add_listener(callback);

        drop(sub);
        event.dispatch(&AlienInfo { id: 0 });

        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn listener_stays_active_while_subscription_is_held() {
        let event: Event<AlienInfo> = Event::new();
        let (count, callback) = counting_alien_listener();
        let _sub = event.add_listener(callback);

        event.dispatch(&AlienInfo { id: 0 });
        event.dispatch(&AlienInfo { id: 0 });

        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn events_of_different_types_are_independent() {
        let mut foo = Foo::new();
        let (alien_count, alien_callback) = counting_alien_listener();
        let zombie_count = Arc::new(AtomicUsize::new(0));
        let z = zombie_count.clone();
        let _alien_sub = foo.on_alien_invasion(alien_callback);
        let _zombie_sub = foo.on_zombie_apocolypse(move |_: &ZombieInfo| {
            z.fetch_add(1, Ordering::SeqCst);
        });
        foo.zombie_apocolypse_active = true;

        foo.check_weather();

        assert_eq!(alien_count.load(Ordering::SeqCst), 0);
        assert_eq!(zombie_count.load(Ordering::SeqCst), 1);
    }

    // Dispatch iterates over a snapshot of the listeners taken when it starts. A listener
    // dropped mid-dispatch is still called once in that dispatch, and never again after.
    #[test]
    fn dropping_subscription_during_dispatch_still_runs_it_this_round() {
        let event: Event<AlienInfo> = Event::new();
        let (count, callback) = counting_alien_listener();
        let slot: Arc<Mutex<Option<Subscription>>> = Arc::new(Mutex::new(None));
        let slot_for_first = slot.clone();
        // Listener A drops subscription B (stored in `slot`) while dispatch is running.
        let _first = event.add_listener(move |_: &AlienInfo| {
            slot_for_first.lock().unwrap().take();
        });
        *slot.lock().unwrap() = Some(event.add_listener(callback));

        // B is still in the snapshot, so it runs once here.
        event.dispatch(&AlienInfo { id: 0 });
        assert_eq!(count.load(Ordering::SeqCst), 1);

        // B has been dropped, so it no longer runs.
        event.dispatch(&AlienInfo { id: 0 });
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    // A listener added mid-dispatch is missing from the snapshot, so it first runs on the
    // next dispatch. The `added` flag makes the adding listener register its child only once.
    #[test]
    fn listener_added_during_dispatch_is_not_called_this_round() {
        let event = Arc::new(Event::<AlienInfo>::new());
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
        event.dispatch(&AlienInfo { id: 0 });
        assert_eq!(count.load(Ordering::SeqCst), 0);

        // C is now registered and runs.
        event.dispatch(&AlienInfo { id: 0 });
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn subscription_outliving_event_does_not_panic() {
        let _sub = {
            let event: Event<AlienInfo> = Event::new();
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
        let _zombie_sub = foo.on_zombie_apocolypse(move |_: &ZombieInfo| {
            z.fetch_add(1, Ordering::SeqCst);
        });
        foo.alien_invasion_active = true;

        foo.check_weather();

        assert_eq!(alien_count.load(Ordering::SeqCst), 1);
        assert_eq!(zombie_count.load(Ordering::SeqCst), 0);
    }
}
