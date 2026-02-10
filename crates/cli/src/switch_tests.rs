// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use tokio::sync::mpsc;

use super::*;

#[tokio::test]
async fn switch_channel_rejects_when_full() -> anyhow::Result<()> {
    let (tx, _rx) = mpsc::channel::<SwitchRequest>(1);

    // Fill the channel
    tx.try_send(SwitchRequest { credentials: None, force: false }).ok();

    // Second send should fail (channel full)
    let result = tx.try_send(SwitchRequest { credentials: None, force: false });

    assert!(result.is_err());
    Ok(())
}
