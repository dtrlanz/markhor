use std::sync::{Arc, atomic::AtomicBool};

use crate::permissions::{Authorized, Permission};

pub struct DependencySlot<T> {
    pub(crate) item: T,
    permissions_granted: Vec<Permission>,
    tracker: Arc<AtomicBool>,
}

impl<T> DependencySlot<T> {
    pub fn new(item: T, permissions_granted: Vec<Permission>) -> Self {
        Self { item, permissions_granted, tracker: Arc::new(AtomicBool::new(false)) }
    }

    pub fn is_active(&self) -> bool {
        Arc::strong_count(&self.tracker) > 1
        || self.tracker.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn tracking_guard(&self) -> TrackingGuard {
        TrackingGuard { tracker: Some(self.tracker.clone()) }
    }

    pub fn reset_tracking(&mut self) {
        self.tracker = Arc::new(AtomicBool::new(false));
    }
}

#[derive(Debug, Clone, Default)]
pub struct TrackingGuard {
    tracker: Option<Arc<AtomicBool>>,
}

impl TrackingGuard {
    pub fn mark_active(&self) {
        if let Some(tracker) = &self.tracker {
            tracker.store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

impl<T> Authorized for DependencySlot<T> {
    fn permissions_granted(&self) -> &[Permission] {
        &self.permissions_granted
    }
}
