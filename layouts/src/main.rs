mod error;
mod herdr;
mod layout;
mod operations;
mod picker;
mod state;

use std::{env, process::ExitCode};

use error::{AppError, AppResult};
use herdr::HerdrClient;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> AppResult<()> {
    let action = env::args()
        .nth(1)
        .ok_or_else(|| AppError::Message("An Arrange action is required".into()))?;
    let client = HerdrClient::from_env()?;

    let result = match action.as_str() {
        "open" => client.open_picker(&active_pane_id()?),
        "picker" => picker::run(&client, &active_pane_id()?),
        "expand" => operations::expand(&client, &active_pane_id()?),
        "balance" => operations::balance(&client, &active_pane_id()?),
        "rotate" => operations::rotate(&client, &active_pane_id()?),
        "undo" => {
            if !operations::undo(&client, &active_pane_id()?)? {
                client.notify("Arrange", "Nothing to undo.");
            }
            Ok(())
        }
        "apply" => {
            let preset_name = env::args()
                .nth(2)
                .ok_or_else(|| AppError::Message("A layout name is required".into()))?;
            let preset = operations::LayoutPreset::from_name(&preset_name)
                .ok_or_else(|| AppError::Message(format!("Unknown layout: {preset_name}")))?;
            operations::apply_preset(&client, &active_pane_id()?, preset)
        }
        _ => Err(AppError::Message(format!(
            "Unknown Arrange action: {action}"
        ))),
    };

    if let Err(error) = &result
        && action != "picker"
    {
        client.notify("Arrange", &error.to_string());
    }

    result
}

fn active_pane_id() -> AppResult<String> {
    env::var("LAYOUT_ORIGIN_PANE_ID")
        .or_else(|_| env::var("HERDR_PANE_ID"))
        .map_err(|_| AppError::Message("The active Herdr pane is unavailable".into()))
}
