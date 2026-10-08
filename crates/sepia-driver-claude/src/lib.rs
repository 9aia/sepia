//! sepia-driver-claude — Claude Code `projects/<slug>/<sessionId>.jsonl`
//! transcript adapter as a driver binary.

pub mod store;
pub mod transcript;

pub use sepia_core::shared::{decode_project_dir, encode_project_dir};
pub use store::{ClaudeStore, TranscriptFile, scan_transcript_files, truncate_claude_transcript};
pub use transcript::{
    ClaudeSourceInfo, from_file, from_jsonl, summarize_jsonl, to_jsonl, tool_file_refs,
    usage_from_claude,
};
