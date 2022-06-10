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

pub mod manager;

use std::borrow::Borrow;
use std::collections::HashSet;

use itertools::Itertools;
use risingwave_hummock_sdk::compaction_group::Prefix;
use risingwave_hummock_sdk::CompactionGroupId;
use risingwave_pb::hummock::CompactionConfig;

use crate::model::MetadataModel;

#[derive(Debug, Clone, PartialEq)]
pub struct CompactionGroup {
    group_id: CompactionGroupId,
    member_prefixes: HashSet<Prefix>,
    compaction_config: CompactionConfig,
}

impl CompactionGroup {
    pub fn new(group_id: CompactionGroupId, compaction_config: CompactionConfig) -> Self {
        Self {
            group_id,
            member_prefixes: Default::default(),
            compaction_config,
        }
    }

    pub fn group_id(&self) -> CompactionGroupId {
        self.group_id
    }

    pub fn member_prefixes(&self) -> &HashSet<Prefix> {
        &self.member_prefixes
    }

    pub fn compaction_config(&self) -> &CompactionConfig {
        &self.compaction_config
    }
}

impl From<&risingwave_pb::hummock::CompactionGroup> for CompactionGroup {
    fn from(compaction_group: &risingwave_pb::hummock::CompactionGroup) -> Self {
        Self {
            group_id: compaction_group.id,
            member_prefixes: compaction_group
                .member_prefixes
                .iter()
                .map(|p| {
                    let u: [u8; 4] = p.clone().try_into().unwrap();
                    u.into()
                })
                .collect(),
            compaction_config: compaction_group
                .compaction_config
                .as_ref()
                .cloned()
                .unwrap(),
        }
    }
}

impl From<&CompactionGroup> for risingwave_pb::hummock::CompactionGroup {
    fn from(compaction_group: &CompactionGroup) -> Self {
        Self {
            id: compaction_group.group_id,
            member_prefixes: compaction_group.member_prefixes.iter().map_into().collect(),
            compaction_config: Some(compaction_group.compaction_config.clone()),
        }
    }
}

const HUMMOCK_COMPACTION_GROUP_CF_NAME: &str = "cf/hummock_compaction_group";

impl MetadataModel for CompactionGroup {
    type KeyType = CompactionGroupId;
    type ProstType = risingwave_pb::hummock::CompactionGroup;

    fn cf_name() -> String {
        String::from(HUMMOCK_COMPACTION_GROUP_CF_NAME)
    }

    fn to_protobuf(&self) -> Self::ProstType {
        self.borrow().into()
    }

    fn from_protobuf(prost: Self::ProstType) -> Self {
        prost.borrow().into()
    }

    fn key(&self) -> risingwave_common::error::Result<Self::KeyType> {
        Ok(self.group_id)
    }
}
