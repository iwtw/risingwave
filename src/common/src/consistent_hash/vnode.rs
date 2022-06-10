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

/// `VirtualNode` is the logical key for consistent hash. Virtual nodes stand for the intermediate
/// layer between data and physical nodes.
pub type VirtualNode = u16;
pub const VNODE_BITS: usize = 11;
pub const VIRTUAL_NODE_COUNT: usize = 1 << VNODE_BITS;
pub const VNODE_BITMAP_LEN: usize = 1 << (VNODE_BITS - 3);

/// `VNodeBitmap` is a bitmap of vnodes for a state table, which indicates the vnodes that the state
/// table owns.
#[derive(Clone, Default, Debug)]
pub struct VNodeBitmap {
    /// Id of state table that the bitmap belongs to.
    table_id: u32,
    /// Bitmap that indicates vnodes that the state tables owns.
    bitmap: Vec<u8>,
}

impl From<risingwave_pb::common::VNodeBitmap> for VNodeBitmap {
    fn from(proto: risingwave_pb::common::VNodeBitmap) -> Self {
        Self {
            table_id: proto.table_id,
            bitmap: proto.bitmap,
        }
    }
}

impl VNodeBitmap {
    pub fn new(table_id: u32, bitmap: Vec<u8>) -> Self {
        Self { table_id, bitmap }
    }

    /// Returns a `VNodeBitmap` with default bitmap. Used for state table of singleton executor.
    pub fn new_with_default_bitmap(table_id: u32) -> Self {
        let mut bitmap = [0u8; VNODE_BITMAP_LEN];
        // Only no.0 vnode is present in default bitmap.
        bitmap[0] = 1;
        Self {
            table_id,
            bitmap: bitmap.to_vec(),
        }
    }

    /// Returns a `VNodeBitmap` with only one vnode present.
    pub fn new_with_single_vnode(table_id: u32, vnode: VirtualNode) -> Self {
        // Make sure the vnode is valid.
        assert!(vnode < VIRTUAL_NODE_COUNT as VirtualNode);
        // Construct vnode bitmap.
        let mut bitmap = [0u8; VNODE_BITMAP_LEN];
        bitmap[(vnode >> 3) as usize] |= 1 << (vnode & 0b111);
        Self {
            table_id,
            bitmap: bitmap.to_vec(),
        }
    }

    /// Checks whether the bitmap overlaps with the given bitmaps. Two bitmaps overlap only when
    /// they belong to the same state table and have at least one common "1" bit. The given
    /// bitmaps are always from `SstableInfo`, which are in the form of protobuf.
    pub fn check_overlap(&self, bitmaps: &[risingwave_pb::common::VNodeBitmap]) -> bool {
        if bitmaps.is_empty() {
            return true;
        }
        if let Ok(pos) =
            bitmaps.binary_search_by_key(&self.table_id, |bitmap| bitmap.get_table_id())
        {
            let candidate_bitmap = &bitmaps[pos];
            assert_eq!(self.bitmap.len(), VNODE_BITMAP_LEN);
            assert_eq!(candidate_bitmap.bitmap.len(), VNODE_BITMAP_LEN);
            for i in 0..VNODE_BITMAP_LEN as usize {
                if (self.bitmap[i] & candidate_bitmap.get_bitmap()[i]) != 0 {
                    return true;
                }
            }
        }
        false
    }

    /// Returns the id of state table that the `VNodeBitmap` belongs to.
    pub fn table_id(&self) -> u32 {
        self.table_id
    }
}

#[cfg(test)]
mod tests {

    use itertools::Itertools;
    use risingwave_pb::common::VNodeBitmap as ProstBitmap;

    use super::*;

    #[test]
    fn test_check_overlap() {
        // The bitmap is for table with id 3, and owns vnodes from 64 to 128.
        let table_id = 3;
        let mut bitmap = [0; VNODE_BITMAP_LEN].to_vec();
        for byte in bitmap.iter_mut().take(16).skip(8) {
            *byte = 0b11111111;
        }
        let vnode_bitmap = VNodeBitmap::new(table_id, bitmap);

        // Test overlap.
        let bitmaps_1 = (2..4)
            .map(|table_id| {
                let mut test_bitmap = [0; VNODE_BITMAP_LEN].to_vec();
                test_bitmap[10] = 0b1;
                ProstBitmap {
                    table_id,
                    bitmap: test_bitmap,
                }
            })
            .collect_vec();
        assert!(vnode_bitmap.check_overlap(&bitmaps_1));

        // Test non-overlap with same table id.
        let bitmaps_2 = (2..4)
            .map(|table_id| {
                let mut test_bitmap = [0; VNODE_BITMAP_LEN].to_vec();
                test_bitmap[20] = 0b1;
                ProstBitmap {
                    table_id,
                    bitmap: test_bitmap,
                }
            })
            .collect_vec();
        assert!(!vnode_bitmap.check_overlap(&bitmaps_2));

        // Test non-overlap with different table ids and same vnodes.
        let bitmaps_3 = (4..6)
            .map(|table_id| {
                let mut test_bitmap = [0; VNODE_BITMAP_LEN].to_vec();
                for byte in test_bitmap.iter_mut().take(16).skip(8) {
                    *byte = 0b11111111;
                }
                ProstBitmap {
                    table_id,
                    bitmap: test_bitmap,
                }
            })
            .collect_vec();
        assert!(!vnode_bitmap.check_overlap(&bitmaps_3));

        // Test empty
        assert!(vnode_bitmap.check_overlap(&[]));
    }
}
