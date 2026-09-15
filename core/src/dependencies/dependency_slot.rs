use std::sync::{Arc, Weak, atomic::{AtomicUsize, Ordering}};

use crate::permissions::{Authorized, Permission};

pub struct DependencySlot<T> {
    pub(crate) item: T,
    permissions_granted: Vec<Permission>,
    tracker: Arc<AtomicUsize>,
}

impl<T> DependencySlot<T> {
    pub fn new(item: T, permissions_granted: Vec<Permission>) -> Self {
        Self { item, permissions_granted, tracker: Arc::new(AtomicUsize::new(0)) }
    }

    pub fn track_one(&self) -> TrackingGuard {
        self.tracker.fetch_add(1, Ordering::AcqRel);
        TrackingGuard { tracker: Some(Arc::downgrade(&self.tracker)) }
    }

    pub fn observer(&self) -> TrackingObserver {
        TrackingObserver { 
            tracker: Arc::clone(&self.tracker)
        }
    }

    pub fn reset_tracking(&mut self) {
        self.tracker = Arc::new(AtomicUsize::new(0));
    }
}

impl<T> Authorized for DependencySlot<T> {
    fn permissions_granted(&self) -> &[Permission] {
        &self.permissions_granted
    }
}

#[derive(Debug, Default)]
pub struct TrackingGuard {
    tracker: Option<Weak<AtomicUsize>>,
}

impl TrackingGuard {
    pub fn mark_active(&self) {
        if let Some(tracker) = &self.tracker {
            if let Some(counter) = tracker.upgrade() {
                counter.fetch_add(1, Ordering::AcqRel);
            }
        }
    }

    pub fn is_tracking(&self) -> bool {
        if let Some(tracker) = &self.tracker {
            if tracker.upgrade().is_some() {
                return true;
            }
        }
        false
    }
}

impl Clone for TrackingGuard {
    fn clone(&self) -> Self {
        if let Some(tracker) = &self.tracker {
            if let Some(counter) = tracker.upgrade() {
                counter.fetch_add(1, Ordering::AcqRel);
            }
        }
        Self { tracker: self.tracker.clone() }
    }
}

impl Drop for TrackingGuard {
    fn drop(&mut self) {
        if let Some(tracker) = &self.tracker {
            if let Some(counter) = tracker.upgrade() {
                counter.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
}

pub struct TrackingObserver {
    tracker: Arc<AtomicUsize>
}

impl TrackingObserver {
    pub fn is_active(&self) -> bool {
        self.tracker.load(Ordering::Acquire) > 0
    }

    pub fn reset(&self) {
        self.tracker.store(0, Ordering::Release);
    }
}
