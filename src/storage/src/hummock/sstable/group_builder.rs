// Copyright 2022 Singularity Data
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use std::future::Future;
use std::mem::size_of;
use std::sync::Arc;

use itertools::Itertools;
use risingwave_hummock_sdk::compaction_group::Prefix;
use risingwave_hummock_sdk::key::{get_table_id, FullKey};
use risingwave_hummock_sdk::{CompactionGroupId, HummockSSTableId};

use crate::hummock::multi_builder::{CapacitySplitTableBuilder, SealedSstableBuilder};
use crate::hummock::sstable_store::SstableStoreRef;
use crate::hummock::value::HummockValue;
use crate::hummock::{HummockResult, SSTableBuilder};

pub type KeyValueGroupId = Vec<u8>;

/// [`KeyValueGroupingImpl`] defines strategies to group key values
#[derive(Clone)]
pub enum KeyValueGroupingImpl {
    VirtualNode(VirtualNodeGrouping),
    CompactionGroup(CompactionGroupGrouping),
    Combined(CombinedGrouping),
}

trait KeyValueGrouping {
    fn group(&self, full_key: &FullKey<&[u8]>, value: &HummockValue<&[u8]>) -> KeyValueGroupId;

    fn group_key_len(&self) -> usize;
}

impl KeyValueGrouping for KeyValueGroupingImpl {
    fn group(&self, full_key: &FullKey<&[u8]>, value: &HummockValue<&[u8]>) -> KeyValueGroupId {
        match self {
            KeyValueGroupingImpl::VirtualNode(grouping) => grouping.group(full_key, value),
            KeyValueGroupingImpl::CompactionGroup(grouping) => grouping.group(full_key, value),
            KeyValueGroupingImpl::Combined(grouping) => grouping.group(full_key, value),
        }
    }

    fn group_key_len(&self) -> usize {
        match self {
            KeyValueGroupingImpl::VirtualNode(grouping) => grouping.group_key_len(),
            KeyValueGroupingImpl::CompactionGroup(grouping) => grouping.group_key_len(),
            KeyValueGroupingImpl::Combined(grouping) => grouping.group_key_len(),
        }
    }
}

/// Groups key value by compaction group
#[derive(Clone)]
pub struct CompactionGroupGrouping {
    prefixes: HashMap<Prefix, CompactionGroupId>,
}

impl CompactionGroupGrouping {
    pub fn new(prefixes: HashMap<Prefix, CompactionGroupId>) -> Self {
        Self { prefixes }
    }
}

impl KeyValueGrouping for CompactionGroupGrouping {
    fn group(&self, full_key: &FullKey<&[u8]>, _value: &HummockValue<&[u8]>) -> KeyValueGroupId {
        let prefix = get_table_id(full_key.inner()).unwrap();
        let group_key = self
            .prefixes
            .get(&prefix.into())
            .cloned()
            .unwrap_or(CompactionGroupId::MAX)
            .to_be_bytes()
            .to_vec();
        debug_assert_eq!(group_key.len(), self.group_key_len());
        group_key
    }

    fn group_key_len(&self) -> usize {
        size_of::<CompactionGroupId>()
    }
}

/// Groups key value by virtual node
#[derive(Clone)]
pub struct VirtualNodeGrouping {
    vnode2unit: Arc<HashMap<u32, Vec<u32>>>,
}

impl VirtualNodeGrouping {
    pub fn new(vnode2unit: Arc<HashMap<u32, Vec<u32>>>) -> Self {
        VirtualNodeGrouping { vnode2unit }
    }
}

impl KeyValueGrouping for VirtualNodeGrouping {
    fn group(&self, full_key: &FullKey<&[u8]>, value: &HummockValue<&[u8]>) -> KeyValueGroupId {
        let group = if let Some(table_id) = get_table_id(full_key.inner()) {
            self.vnode2unit.get(&table_id).map(|mapping| {
                let idx = match value {
                    HummockValue::Put(meta, _) => meta.vnode,
                    HummockValue::Delete(meta) => meta.vnode,
                };
                mapping[idx as usize]
            })
        } else {
            None
        };
        let group_key = group.unwrap_or(u32::MAX).to_be_bytes().to_vec();
        debug_assert_eq!(group_key.len(), self.group_key_len());
        group_key
    }

    fn group_key_len(&self) -> usize {
        size_of::<u32>()
    }
}

#[derive(Clone)]
pub struct CombinedGrouping {
    group_impls: Vec<KeyValueGroupingImpl>,
}

impl CombinedGrouping {
    pub fn new(group_impls: Vec<KeyValueGroupingImpl>) -> Self {
        Self { group_impls }
    }
}

impl KeyValueGrouping for CombinedGrouping {
    fn group(&self, full_key: &FullKey<&[u8]>, value: &HummockValue<&[u8]>) -> KeyValueGroupId {
        let group_key = self
            .group_impls
            .iter()
            .flat_map(|g| g.group(full_key, value))
            .collect_vec();
        debug_assert_eq!(group_key.len(), self.group_key_len());
        group_key
    }

    fn group_key_len(&self) -> usize {
        self.group_impls.iter().map(|g| g.group_key_len()).sum()
    }
}

/// A wrapper for [`CapacitySplitTableBuilder`] which automatically split key-value pairs into
/// multiple `SSTables`, based on [`KeyValueGroupingImpl`].
pub struct GroupedSstableBuilder<B> {
    /// See [`CapacitySplitTableBuilder`]
    get_id_and_builder: B,
    grouping: KeyValueGroupingImpl,
    builders: HashMap<KeyValueGroupId, CapacitySplitTableBuilder<B>>,
    sstable_store: SstableStoreRef,
}

impl<B, F> GroupedSstableBuilder<B>
where
    B: Clone + Fn() -> F,
    F: Future<Output = HummockResult<(HummockSSTableId, SSTableBuilder)>>,
{
    pub fn new(
        get_id_and_builder: B,
        grouping: KeyValueGroupingImpl,
        sstable_store: SstableStoreRef,
    ) -> Self {
        Self {
            get_id_and_builder,
            grouping,
            builders: Default::default(),
            sstable_store,
        }
    }

    pub fn len(&self) -> usize {
        self.builders.iter().map(|(_k, v)| v.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.builders.iter().all(|(_k, v)| v.is_empty())
    }

    pub async fn add_full_key(
        &mut self,
        full_key: FullKey<&[u8]>,
        value: HummockValue<&[u8]>,
        allow_split: bool,
    ) -> HummockResult<()> {
        let group_id = self.grouping.group(&full_key, &value);
        let entry = self.builders.entry(group_id).or_insert_with(|| {
            CapacitySplitTableBuilder::new(
                self.get_id_and_builder.clone(),
                self.sstable_store.clone(),
            )
        });
        entry.add_full_key(full_key, value, allow_split).await
    }

    pub fn seal_current(&mut self) {
        self.builders
            .iter_mut()
            .for_each(|(_k, v)| v.seal_current());
    }

    pub fn finish(mut self) -> Vec<SealedSstableBuilder> {
        self.seal_current();
        self.builders
            .into_iter()
            .flat_map(|(_k, v)| v.finish())
            .collect_vec()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering::SeqCst;

    use bytes::Buf;
    use risingwave_common::consistent_hash::VirtualNode;

    use super::*;
    use crate::hummock::iterator::test_utils::mock_sstable_store;
    use crate::hummock::sstable::utils::CompressionAlgorithm;
    use crate::hummock::{SSTableBuilderOptions, DEFAULT_RESTART_INTERVAL};
    use crate::storage_value::ValueMeta;

    #[tokio::test]
    async fn test_compaction_group_grouping() {
        let next_id = AtomicU64::new(1001);
        let block_size = 1 << 10;
        let table_capacity = 4 * block_size;
        let get_id_and_builder = || async {
            Ok((
                next_id.fetch_add(1, SeqCst),
                SSTableBuilder::new(SSTableBuilderOptions {
                    capacity: table_capacity,
                    block_capacity: block_size,
                    restart_interval: DEFAULT_RESTART_INTERVAL,
                    bloom_false_positive: 0.1,
                    compression_algorithm: CompressionAlgorithm::None,
                }),
            ))
        };
        let prefix = b"\x01\x02\x03\x04".as_slice().get_u32();
        // one compaction group defined
        let grouping = KeyValueGroupingImpl::CompactionGroup(CompactionGroupGrouping::new(
            HashMap::from([(prefix.into(), 1 as CompactionGroupId)]),
        ));
        let mut builder =
            GroupedSstableBuilder::new(get_id_and_builder, grouping, mock_sstable_store());
        for i in 0..10 {
            // key value belongs to no compaction group
            builder
                .add_full_key(
                    FullKey::from_user_key(
                        b"\x74\x00\x00\x00\x00".to_vec(),
                        (table_capacity - i) as u64,
                    )
                    .as_slice(),
                    HummockValue::put(b"value"),
                    false,
                )
                .await
                .unwrap();
            // key value belongs to compaction group `prefix`
            builder
                .add_full_key(
                    FullKey::from_user_key(
                        [b"\x74", prefix.to_be_bytes().as_slice()].concat().to_vec(),
                        (table_capacity - i) as u64,
                    )
                    .as_slice(),
                    HummockValue::put(b"value"),
                    false,
                )
                .await
                .unwrap();
        }
        builder.seal_current();
        let results = builder.finish();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_virtual_node_grouping() {
        let next_id = AtomicU64::new(1001);
        let block_size = 1 << 10;
        let table_capacity = 4 * block_size;
        let get_id_and_builder = || async {
            Ok((
                next_id.fetch_add(1, SeqCst),
                SSTableBuilder::new(SSTableBuilderOptions {
                    capacity: table_capacity,
                    block_capacity: block_size,
                    restart_interval: DEFAULT_RESTART_INTERVAL,
                    bloom_false_positive: 0.1,
                    compression_algorithm: CompressionAlgorithm::None,
                }),
            ))
        };
        let table_id = 1u32;
        let vnode_number = 10u32;
        let parallel_unit_number = 3u32;
        // See Keyspace::table_root
        let magic_prefix = b't';
        let vnode2unit = Arc::new(HashMap::from([(
            table_id,
            { 0..vnode_number }
                .into_iter()
                .map(|i| i % parallel_unit_number)
                .collect_vec(),
        )]));

        // Test < parallel_unit_number
        let grouping = KeyValueGroupingImpl::VirtualNode(VirtualNodeGrouping::new(vnode2unit));
        let mut builder =
            GroupedSstableBuilder::new(get_id_and_builder, grouping.clone(), mock_sstable_store());
        for i in 0..2 {
            // key value matches table and mapping
            builder
                .add_full_key(
                    FullKey::from_user_key(
                        [&[magic_prefix], table_id.to_be_bytes().as_slice()]
                            .concat()
                            .to_vec(),
                        (table_capacity - i) as u64,
                    )
                    .as_slice(),
                    HummockValue::Put(ValueMeta::with_vnode(i as VirtualNode), b"value"),
                    false,
                )
                .await
                .unwrap();
        }
        builder.seal_current();
        let results = builder.finish();
        assert_eq!(results.len(), 2);
        assert!(results.len() < parallel_unit_number as usize);

        // Test > parallel_unit_number
        let mut builder =
            GroupedSstableBuilder::new(get_id_and_builder, grouping.clone(), mock_sstable_store());
        for i in 0..10 {
            // key value matches table and mapping
            builder
                .add_full_key(
                    FullKey::from_user_key(
                        [&[magic_prefix], table_id.to_be_bytes().as_slice()]
                            .concat()
                            .to_vec(),
                        (table_capacity - i) as u64,
                    )
                    .as_slice(),
                    HummockValue::Put(ValueMeta::with_vnode(i as VirtualNode), b"value"),
                    false,
                )
                .await
                .unwrap();
        }
        builder.seal_current();
        let results = builder.finish();
        assert_eq!(results.len(), parallel_unit_number as usize);

        // Test no matching table id
        let mut builder =
            GroupedSstableBuilder::new(get_id_and_builder, grouping.clone(), mock_sstable_store());
        builder
            .add_full_key(
                FullKey::from_user_key(
                    [&[magic_prefix], (2u32).to_be_bytes().as_slice()]
                        .concat()
                        .to_vec(),
                    table_capacity as u64,
                )
                .as_slice(),
                HummockValue::Put(ValueMeta::with_vnode(0 as VirtualNode), b"value"),
                false,
            )
            .await
            .unwrap();
        builder.seal_current();
        let results = builder.finish();
        assert_eq!(results.len(), 1usize);
    }

    #[tokio::test]
    async fn test_combined_grouping() {
        let next_id = AtomicU64::new(1001);
        let block_size = 1 << 10;
        let table_capacity = 4 * block_size;
        let get_id_and_builder = || async {
            Ok((
                next_id.fetch_add(1, SeqCst),
                SSTableBuilder::new(SSTableBuilderOptions {
                    capacity: table_capacity,
                    block_capacity: block_size,
                    restart_interval: DEFAULT_RESTART_INTERVAL,
                    bloom_false_positive: 0.1,
                    compression_algorithm: CompressionAlgorithm::None,
                }),
            ))
        };

        let compaction_group_grouping =
            KeyValueGroupingImpl::CompactionGroup(CompactionGroupGrouping::new(HashMap::from([
                (0u32.into(), 1 as CompactionGroupId),
                (1u32.into(), 2 as CompactionGroupId),
            ])));
        let magic_prefix = b't';
        let vnode2unit = Arc::new(HashMap::from([(0u32, vec![0, 1]), (1u32, vec![0, 1])]));
        let virtual_node_grouping =
            KeyValueGroupingImpl::VirtualNode(VirtualNodeGrouping::new(vnode2unit));
        let grouping = KeyValueGroupingImpl::Combined(CombinedGrouping::new(vec![
            compaction_group_grouping,
            virtual_node_grouping,
        ]));
        let mut builder =
            GroupedSstableBuilder::new(get_id_and_builder, grouping, mock_sstable_store());
        for i in 0..4 {
            builder
                .add_full_key(
                    FullKey::from_user_key(
                        [&[magic_prefix], ((i % 2) as u32).to_be_bytes().as_slice()]
                            .concat()
                            .to_vec(),
                        (table_capacity - i) as u64,
                    )
                    .as_slice(),
                    HummockValue::Put(ValueMeta::with_vnode((i / 2) as VirtualNode), b"value"),
                    false,
                )
                .await
                .unwrap();
        }
        builder.seal_current();
        let results = builder.finish();
        assert_eq!(results.len(), 4usize);
    }
}
