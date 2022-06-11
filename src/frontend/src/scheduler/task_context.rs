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

use std::sync::Arc;

use risingwave_batch::executor::BatchMetrics;
use risingwave_batch::task::{BatchTaskContext, TaskOutput, TaskOutputId};
use risingwave_common::error::Result;
use risingwave_common::util::addr::{is_local_address, HostAddr};
use risingwave_source::SourceManagerRef;

use crate::session::FrontendEnv;

/// Batch task execution context in frontend.
#[derive(Clone)]
pub struct FrontendBatchTaskContext {
    env: FrontendEnv,
}

impl FrontendBatchTaskContext {
    pub fn new(env: FrontendEnv) -> Self {
        Self { env }
    }
}

impl BatchTaskContext for FrontendBatchTaskContext {
    fn get_task_output(&self, _task_output_id: TaskOutputId) -> Result<TaskOutput> {
        todo!()
    }

    fn is_local_addr(&self, peer_addr: &HostAddr) -> bool {
        is_local_address(self.env.server_address(), peer_addr)
    }

    fn source_manager_ref(&self) -> Option<SourceManagerRef> {
        todo!()
    }

    fn state_store(&self) -> Option<risingwave_storage::store_impl::StateStoreImpl> {
        todo!()
    }

    fn stats(&self) -> Arc<BatchMetrics> {
        todo!()
    }
}
