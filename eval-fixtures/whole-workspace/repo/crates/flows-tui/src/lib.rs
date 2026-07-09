//! flows-tui — ratatui terminal UI: the launcher home screen, the live run
//! view, and the reattach view (all sharing one `App`).

pub mod app;
pub mod attach;
pub mod home;
pub mod output;
pub mod phases;
pub mod render;
pub mod run;

pub use app::{App, ChannelCallbacks, UiEvent};
pub use attach::attach_tui;
pub use home::{home_screen, plan_screen, HomeAction};
pub use output::{Category, OutputPane};
pub use phases::{PhasePanel, PhaseStatus};
pub use run::run_tui;
