// SPDX-License-Identifier: GPL-3.0-or-later
// `db` subcommand.

use crate::cli::output::Output;
use crate::cli::root::DbCmd;
use crate::error::CliError;
use crate::ipc::storage::{db, tasks};

pub async fn run(cmd: DbCmd, out: &Output) -> Result<(), CliError> {
    match cmd {
        DbCmd::Export { file } => {
            db::export(file).await?;
            out.status("database exported")
        }
        DbCmd::Import { file } => {
            db::import(file).await?;
            out.status("database imported")
        }
        DbCmd::Tasks => {
            let all = tasks::load().await?;
            out.list(
                all.into_iter()
                    .map(|t| {
                        serde_json::json!({
                            "id": t.id,
                            "type": t.task_type.as_str(),
                            "source": t.source,
                            "status": t.status.as_str(),
                            "progress": t.progress,
                        })
                    })
                    .collect(),
            )
        }
    }
}
