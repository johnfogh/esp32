//! Self-contained static HTML report builder.
//!
//! Generated reports are a single HTML file with inline CSS, inline JavaScript,
//! and images encoded as `data:` base64 URIs. Library markup does not reference
//! external files.

mod asset;
mod html;
mod report;

pub use asset::{data_uri, Mime};
pub use report::{Block, Report, TargetCard, TimelineEvent, TimelineKind};
