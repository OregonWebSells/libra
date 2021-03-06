// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::{
    backup_types::epoch_ending::manifest::EpochEndingBackup,
    storage::{BackupStorage, FileHandle},
    utils::read_record_bytes::ReadRecordBytes,
};
use anyhow::{anyhow, ensure, Result};
use libra_types::{ledger_info::LedgerInfoWithSignatures, waypoint::Waypoint};
use libradb::backup::restore_handler::RestoreHandler;
use std::sync::Arc;
use structopt::StructOpt;
use tokio::io::AsyncReadExt;

#[derive(StructOpt)]
pub struct EpochEndingRestoreOpt {
    #[structopt(long = "state-manifest")]
    pub manifest_handle: FileHandle,
}

pub struct EpochEndingRestoreController {
    storage: Arc<dyn BackupStorage>,
    restore_handler: Arc<RestoreHandler>,
    manifest_handle: FileHandle,
}

impl EpochEndingRestoreController {
    pub fn new(
        opt: EpochEndingRestoreOpt,
        storage: Arc<dyn BackupStorage>,
        restore_handler: Arc<RestoreHandler>,
    ) -> Self {
        Self {
            storage,
            restore_handler,
            manifest_handle: opt.manifest_handle,
        }
    }

    pub async fn run(self) -> Result<()> {
        let mut manifest_bytes = Vec::new();
        self.storage
            .open_for_read(&self.manifest_handle)
            .await?
            .read_to_end(&mut manifest_bytes)
            .await?;
        let manifest: EpochEndingBackup = serde_json::from_slice(&manifest_bytes)?;
        manifest.verify()?;

        let mut next_epoch = manifest.first_epoch;
        let mut waypoint_iter = manifest.waypoints.iter();

        for chunk in manifest.chunks {
            let lis = self.read_chunk(chunk.ledger_infos).await?;
            ensure!(
                chunk.first_epoch + lis.len() as u64 == chunk.last_epoch + 1,
                "Number of items in chunks doesn't match that in manifest. first_epoch: {}, last_epoch: {}, items in chunk: {}",
                chunk.first_epoch,
                chunk.last_epoch,
                lis.len(),
            );
            // verify
            for li in lis.iter() {
                ensure!(
                    li.ledger_info().epoch() == next_epoch,
                    "LedgerInfo epoch not expected. Expected: {}, actual: {}.",
                    li.ledger_info().epoch(),
                    next_epoch,
                );
                let wp_manifest = waypoint_iter.next().ok_or_else(|| {
                    anyhow!("More LedgerInfo's found than waypoints in manifest.")
                })?;
                let wp_li = Waypoint::new_epoch_boundary(li.ledger_info())?;
                // TODO: verify signature on li
                ensure!(
                    *wp_manifest == wp_li,
                    "Waypoints don't match. In manifest: {}, In chunk: {}",
                    wp_manifest,
                    wp_li,
                );
                next_epoch += 1;
            }

            // write to db
            self.restore_handler.save_ledger_infos(&lis)?;
        }

        Ok(())
    }
}

impl EpochEndingRestoreController {
    async fn read_chunk(&self, file_handle: FileHandle) -> Result<Vec<LedgerInfoWithSignatures>> {
        let mut file = self.storage.open_for_read(&file_handle).await?;
        let mut chunk = vec![];

        while let Some(record_bytes) = file.read_record_bytes().await? {
            chunk.push(lcs::from_bytes(&record_bytes)?);
        }

        Ok(chunk)
    }
}
