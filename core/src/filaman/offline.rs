use alloc::{format, string::String, vec::Vec};
use core::cell::RefCell;

use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use crate::spool::FilamentSpool;

use super::{FilaManLocation, SpoolRegistration, canonical_external_id};

pub(super) const SCHEMA_VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct CachedSpool {
    pub external_id: String,
    pub spool_id: u64,
    pub location_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct InventorySnapshot {
    schema: u8,
    locations: Vec<FilaManLocation>,
    spools: Vec<CachedSpool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PendingStorage {
    pub sequence: u64,
    pub spool: FilamentSpool,
    pub location_id: u64,
    pub location_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OfflineQueue {
    schema: u8,
    next_sequence: u64,
    operations: Vec<PendingStorage>,
}

impl Default for OfflineQueue {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            next_sequence: 1,
            operations: Vec::new(),
        }
    }
}

pub(super) struct PreparedInventory {
    snapshot: InventorySnapshot,
    pub serialized: Vec<u8>,
    pub changed: bool,
}

impl PreparedInventory {
    pub fn spool_count(&self) -> usize {
        self.snapshot.spools.len()
    }

    pub fn location_count(&self) -> usize {
        self.snapshot.locations.len()
    }
}

pub(super) struct PreparedQueue {
    queue: OfflineQueue,
    pub serialized: Vec<u8>,
    pub changed: bool,
}

pub(super) struct OfflineStatus {
    pub cached_spools: usize,
    pub cached_locations: usize,
    pub pending_operations: usize,
}

pub(super) struct OfflineState {
    inventory: RefCell<Option<InventorySnapshot>>,
    inventory_index: RefCell<HashMap<String, CachedSpool>>,
    inventory_serialized: RefCell<Vec<u8>>,
    queue: RefCell<OfflineQueue>,
    queue_serialized: RefCell<Vec<u8>>,
}

impl OfflineState {
    pub fn new() -> Self {
        Self {
            inventory: RefCell::new(None),
            inventory_index: RefCell::new(HashMap::new()),
            inventory_serialized: RefCell::new(Vec::new()),
            queue: RefCell::new(OfflineQueue::default()),
            queue_serialized: RefCell::new(Vec::new()),
        }
    }

    pub fn status(&self) -> OfflineStatus {
        let inventory = self.inventory.borrow();
        OfflineStatus {
            cached_spools: inventory.as_ref().map(|snapshot| snapshot.spools.len()).unwrap_or(0),
            cached_locations: inventory.as_ref().map(|snapshot| snapshot.locations.len()).unwrap_or(0),
            pending_operations: self.queue.borrow().operations.len(),
        }
    }

    pub fn has_inventory(&self) -> bool {
        self.inventory.borrow().is_some()
    }

    pub fn load_inventory(&self, bytes: Vec<u8>) -> Result<OfflineStatus, String> {
        let snapshot: InventorySnapshot = serde_json::from_slice(&bytes).map_err(|error| format!("inventory deserialization failed: {error}"))?;
        if snapshot.schema != SCHEMA_VERSION {
            return Err("unsupported inventory schema".into());
        }
        self.commit_inventory(PreparedInventory {
            snapshot,
            serialized: bytes,
            changed: false,
        });
        Ok(self.status())
    }

    pub fn load_queue(&self, bytes: Vec<u8>) -> Result<usize, String> {
        let queue: OfflineQueue = serde_json::from_slice(&bytes).map_err(|error| format!("queue deserialization failed: {error}"))?;
        if queue.schema != SCHEMA_VERSION {
            return Err("unsupported queue schema".into());
        }
        let count = queue.operations.len();
        *self.queue.borrow_mut() = queue;
        *self.queue_serialized.borrow_mut() = bytes;
        Ok(count)
    }

    pub fn resolve(&self, spool: &FilamentSpool, offline: bool) -> SpoolRegistration {
        let external_id = canonical_external_id(spool);
        let locations = self
            .inventory
            .borrow()
            .as_ref()
            .map(|snapshot| snapshot.locations.clone())
            .unwrap_or_default();

        if let Some(operation) = self
            .queue
            .borrow()
            .operations
            .iter()
            .find(|operation| canonical_external_id(&operation.spool) == external_id)
        {
            return SpoolRegistration::Pending {
                location_id: operation.location_id,
                location_name: operation.location_name.clone(),
                locations,
            };
        }

        if let Some(existing) = self.inventory_index.borrow().get(&external_id).cloned() {
            let location_name = existing.location_id.and_then(|location_id| {
                locations
                    .iter()
                    .find(|location| location.id == location_id)
                    .map(|location| location.name.clone())
            });
            return SpoolRegistration::Existing {
                spool_id: existing.spool_id,
                location_id: existing.location_id,
                location_name,
                locations,
                offline,
            };
        }

        SpoolRegistration::New {
            locations,
            offline,
            inventory_known: self.inventory.borrow().is_some(),
        }
    }

    pub fn prepare_inventory(&self, mut locations: Vec<FilaManLocation>, mut spools: Vec<CachedSpool>) -> Result<PreparedInventory, String> {
        locations.sort_by_key(|location| location.id);
        locations.dedup_by_key(|location| location.id);
        spools.sort_by(|left, right| left.external_id.cmp(&right.external_id));
        spools.dedup_by(|left, right| left.external_id == right.external_id);
        let snapshot = InventorySnapshot {
            schema: SCHEMA_VERSION,
            locations,
            spools,
        };
        let serialized = serde_json::to_vec(&snapshot).map_err(|error| format!("inventory serialization failed: {error}"))?;
        let changed = serialized.as_slice() != self.inventory_serialized.borrow().as_slice();
        Ok(PreparedInventory {
            snapshot,
            serialized,
            changed,
        })
    }

    pub fn commit_inventory(&self, prepared: PreparedInventory) {
        let mut index = HashMap::with_capacity(prepared.snapshot.spools.len());
        for spool in &prepared.snapshot.spools {
            index.insert(spool.external_id.clone(), spool.clone());
        }
        *self.inventory_index.borrow_mut() = index;
        *self.inventory.borrow_mut() = Some(prepared.snapshot);
        *self.inventory_serialized.borrow_mut() = prepared.serialized;
    }

    pub fn operations(&self) -> Vec<PendingStorage> {
        self.queue.borrow().operations.clone()
    }

    pub fn prepare_storage(&self, spool: &FilamentSpool, location_id: u64, location_name: &str) -> Result<PreparedQueue, String> {
        let external_id = canonical_external_id(spool);
        let mut queue = self.queue.borrow().clone();
        if let Some(existing) = queue
            .operations
            .iter_mut()
            .find(|operation| canonical_external_id(&operation.spool) == external_id)
        {
            existing.spool = spool.clone();
            existing.location_id = location_id;
            existing.location_name = location_name.into();
        } else {
            let sequence = queue.next_sequence;
            queue.next_sequence = queue.next_sequence.saturating_add(1);
            queue.operations.push(PendingStorage {
                sequence,
                spool: spool.clone(),
                location_id,
                location_name: location_name.into(),
            });
        }
        self.prepare_queue(queue)
    }

    pub fn prepare_remaining(&self, operations: Vec<PendingStorage>) -> Result<PreparedQueue, String> {
        let mut queue = self.queue.borrow().clone();
        queue.operations = operations;
        self.prepare_queue(queue)
    }

    fn prepare_queue(&self, queue: OfflineQueue) -> Result<PreparedQueue, String> {
        let serialized = serde_json::to_vec(&queue).map_err(|error| format!("queue serialization failed: {error}"))?;
        let changed = serialized.as_slice() != self.queue_serialized.borrow().as_slice();
        Ok(PreparedQueue { queue, serialized, changed })
    }

    pub fn commit_queue(&self, prepared: PreparedQueue) {
        *self.queue.borrow_mut() = prepared.queue;
        *self.queue_serialized.borrow_mut() = prepared.serialized;
    }
}
