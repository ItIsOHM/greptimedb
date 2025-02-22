// Copyright 2023 Greptime Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

mod instance_test;
mod promql_test;
mod test_util;

use std::collections::HashMap;
use std::sync::Arc;

use catalog::RegisterSchemaRequest;
use common_test_util::temp_dir::TempDir;
use datanode::instance::Instance as DatanodeInstance;
use frontend::instance::Instance;
use table::engine::{region_name, table_dir};

use crate::cluster::{GreptimeDbCluster, GreptimeDbClusterBuilder};
use crate::test_util::{create_tmp_dir_and_datanode_opts, StorageType, TempDirGuard, TestGuard};

pub struct MockDistributedInstance(GreptimeDbCluster);

impl MockDistributedInstance {
    pub fn data_tmp_dirs(&self) -> Vec<&TempDir> {
        self.0
            .storage_guards
            .iter()
            .map(|g| {
                let TempDirGuard::File(dir) = &g.0 else { unreachable!() };
                dir
            })
            .collect()
    }

    pub fn frontend(&self) -> Arc<Instance> {
        self.0.frontend.clone()
    }

    pub fn datanodes(&self) -> &HashMap<u64, Arc<DatanodeInstance>> {
        &self.0.datanode_instances
    }
}

pub struct MockStandaloneInstance {
    pub instance: Arc<Instance>,
    _guard: TestGuard,
}

impl MockStandaloneInstance {
    pub fn data_tmp_dir(&self) -> &TempDir {
        let TempDirGuard::File(dir) = &self._guard.storage_guard.0 else { unreachable!() };
        dir
    }
}

pub(crate) async fn create_standalone_instance(test_name: &str) -> MockStandaloneInstance {
    let (opts, guard) = create_tmp_dir_and_datanode_opts(StorageType::File, test_name);
    let (dn_instance, heartbeat) = DatanodeInstance::with_opts(&opts, Default::default())
        .await
        .unwrap();

    let frontend_instance = Instance::try_new_standalone(dn_instance.clone())
        .await
        .unwrap();

    assert!(dn_instance
        .catalog_manager()
        .register_catalog("another_catalog".to_string())
        .await
        .is_ok());
    let req = RegisterSchemaRequest {
        catalog: "another_catalog".to_string(),
        schema: "another_schema".to_string(),
    };
    assert!(dn_instance
        .catalog_manager()
        .register_schema(req)
        .await
        .is_ok());

    dn_instance.start().await.unwrap();
    if let Some(heartbeat) = heartbeat {
        heartbeat.start().await.unwrap();
    };
    MockStandaloneInstance {
        instance: Arc::new(frontend_instance),
        _guard: guard,
    }
}

pub async fn create_distributed_instance(test_name: &str) -> MockDistributedInstance {
    let cluster = GreptimeDbClusterBuilder::new(test_name).build().await;
    MockDistributedInstance(cluster)
}

pub fn test_region_dir(
    dir: &str,
    catalog_name: &str,
    schema_name: &str,
    table_id: u32,
    region_id: u32,
) -> String {
    let table_dir = table_dir(catalog_name, schema_name, table_id);
    let region_name = region_name(table_id, region_id);

    format!("{}/{}/{}", dir, table_dir, region_name)
}

pub fn has_parquet_file(sst_dir: &str) -> bool {
    for entry in std::fs::read_dir(sst_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if !path.is_dir() {
            assert_eq!("parquet", path.extension().unwrap());
            return true;
        }
    }

    false
}
