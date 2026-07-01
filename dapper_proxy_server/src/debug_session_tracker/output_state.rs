// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::collections::VecDeque;
use std::fs::File;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use dapper_control_api::BufferedOutput;
use dapper_control_api::OutputEvent;
use dapper_dap_protocol::data_types::Seq;
use dapper_dap_protocol::enums::OutputCategory;
use dapper_session::SessionId;

/// Returns the directory for dapper output logs (in the system temp directory)
fn get_dapper_output_dir() -> PathBuf {
    dapper_session::get_user_temp_dir().join("output")
}

/// Encapsulates output messages from the debugger (stdout, stderr, console).
/// Writes output to a file so agents can read more if needed.
/// Maintains a bounded buffer of output events that is cleared after each context response.
/// The buffer retains at most `max_buffer_size` events, split into head (earliest)
/// and tail (most recent) halves, matching the display pattern used in the context footer.
#[derive(Debug)]
pub struct OutputState {
    output_file_path: PathBuf,
    writer: Option<BufWriter<File>>,
    disabled: bool,
    max_buffer_size: usize,
    head: Vec<OutputEvent>,
    tail: VecDeque<OutputEvent>,
    total_count: usize,
}

impl OutputState {
    pub fn new(session_id: &SessionId, max_output_lines: usize) -> Self {
        let output_dir = get_dapper_output_dir();
        let output_file_path = output_dir.join(format!("{}.log", session_id));
        Self {
            output_file_path,
            writer: None,
            disabled: max_output_lines == 0,
            max_buffer_size: max_output_lines,
            head: Vec::with_capacity(max_output_lines / 2),
            tail: VecDeque::with_capacity(max_output_lines - max_output_lines / 2),
            total_count: 0,
        }
    }

    fn initialize(&mut self) -> std::io::Result<()> {
        if self.disabled || self.writer.is_some() {
            return Ok(());
        }

        if let Some(parent) = self.output_file_path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            self.disabled = true;
            return Err(e);
        }

        match File::create(&self.output_file_path) {
            Ok(file) => {
                self.writer = Some(BufWriter::new(file));
                Ok(())
            }
            Err(e) => {
                self.disabled = true;
                Err(e)
            }
        }
    }

    fn flush(&mut self) {
        if let Some(writer) = self.writer.as_mut() {
            let _ = writer.flush();
        }
    }

    /// Add output to the file. Silently succeeds if output tracking is disabled.
    /// Uses the real DAP event seq number for tracking.
    pub fn add_output(
        &mut self,
        output: &str,
        category: Option<&OutputCategory>,
        dap_seq: Seq,
    ) -> std::io::Result<()> {
        if self.disabled {
            return Ok(());
        }

        self.initialize()?;

        let event = OutputEvent {
            seq: dap_seq,
            category: category.cloned(),
            output: output.to_string(),
            ..Default::default()
        };

        let head_capacity = self.max_buffer_size / 2;
        let tail_capacity = self.max_buffer_size - head_capacity;

        if head_capacity > 0 && self.head.len() < head_capacity {
            self.head.push(event);
        } else if tail_capacity > 0 {
            if self.tail.len() >= tail_capacity {
                self.tail.pop_front();
            }
            self.tail.push_back(event);
        }

        self.total_count += 1;

        if let Some(writer) = self.writer.as_mut() {
            let category_str = category.map(|c| c.as_ref()).unwrap_or("unspecified");
            write!(writer, "[seq:{} {}] {}", dap_seq, category_str, output)?;
        }
        Ok(())
    }

    /// Returns the path to the output file
    pub fn output_file_path(&self) -> &Path {
        &self.output_file_path
    }

    pub fn has_buffered_output(&self) -> bool {
        self.total_count > 0
    }

    pub fn take_buffered_output(&mut self) -> BufferedOutput {
        self.flush();
        let total_count = self.total_count;
        self.total_count = 0;
        BufferedOutput {
            head: self.head.drain(..).collect(),
            tail: self.tail.drain(..).collect(),
            total_count,
            ..Default::default()
        }
    }
}

#[cfg(test)]
impl OutputState {
    pub fn buffer_len(&self) -> usize {
        self.total_count
    }

    /// Returns true if there is any output (file exists and has content)
    pub fn has_any(&mut self) -> bool {
        self.flush();
        self.output_file_path
            .metadata()
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    }

    pub fn read_last_lines(&mut self, max_lines: usize) -> std::io::Result<String> {
        self.flush();

        let content = std::fs::read_to_string(&self.output_file_path)?;
        let lines: Vec<&str> = content.lines().collect();

        let start_idx = lines.len().saturating_sub(max_lines);

        Ok(lines[start_idx..].join("\n"))
    }

    pub fn cleanup(&mut self) {
        self.writer = None;
        if self.output_file_path.exists()
            && let Err(e) = std::fs::remove_file(&self.output_file_path)
        {
            tracing::warn!(
                error = %e,
                path = %self.output_file_path.display(),
                "Failed to cleanup output file"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use dapper_dap_protocol::data_types::Seq;

    use super::*;

    #[test]
    fn test_output_state_file_operations() {
        let session_id = make_test_session_id("file-ops");
        let mut output_state = OutputState::new(&session_id, 20);

        // Write multiple lines with different DAP seq numbers
        output_state
            .add_output("Line 1\n", Some(&OutputCategory::Stdout), Seq(100))
            .unwrap();
        output_state
            .add_output("Line 2\n", Some(&OutputCategory::Stdout), Seq(101))
            .unwrap();
        output_state
            .add_output("Line 3\n", Some(&OutputCategory::Stdout), Seq(102))
            .unwrap();

        // Verify file exists and has content
        assert!(output_state.has_any());

        // Read last lines
        let content = output_state.read_last_lines(10).unwrap();
        assert!(content.contains("Line 1"));
        assert!(content.contains("Line 2"));
        assert!(content.contains("Line 3"));

        // Cleanup
        output_state.cleanup();
        assert!(!output_state.output_file_path().exists());
    }

    #[test]
    fn test_output_state_read_last_lines() {
        let session_id = make_test_session_id("last-lines");
        let mut output_state = OutputState::new(&session_id, 20);

        // Write 5 lines in a single output event
        output_state
            .add_output(
                "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\n",
                Some(&OutputCategory::Stdout),
                Seq(100),
            )
            .unwrap();

        // Read last 3 lines
        let content = output_state.read_last_lines(3).unwrap();
        assert!(!content.contains("Line 1"));
        assert!(!content.contains("Line 2"));
        assert!(content.contains("Line 3"));
        assert!(content.contains("Line 4"));
        assert!(content.contains("Line 5"));

        // Read all lines (more than available)
        let content = output_state.read_last_lines(10).unwrap();
        assert!(content.contains("Line 1"));
        assert!(content.contains("Line 5"));

        // Cleanup
        output_state.cleanup();
    }

    #[test]
    fn test_output_state_disabled_when_max_lines_zero() {
        let session_id = make_test_session_id("disabled");
        let mut output_state = OutputState::new(&session_id, 0);

        // Should silently succeed when disabled (no error, no file created)
        let result = output_state.add_output("This should not be written\n", None, Seq(100));
        assert!(result.is_ok());

        // File should not exist since disabled
        assert!(!output_state.output_file_path().exists());
    }

    #[test]
    fn test_take_buffered_output_within_capacity() {
        let session_id = make_test_session_id("take-buf");
        let mut output_state = OutputState::new(&session_id, 20);

        for i in 1..=5 {
            output_state
                .add_output(
                    &format!("Event {}\n", i),
                    Some(&OutputCategory::Stdout),
                    Seq(i as i64),
                )
                .unwrap();
        }

        let buffered = output_state.take_buffered_output();
        assert_eq!(buffered.total_count, 5);
        assert_eq!(buffered.head.len(), 5);
        assert!(buffered.tail.is_empty());
        assert_eq!(buffered.head[0].output, "Event 1\n");
        assert_eq!(buffered.head[4].output, "Event 5\n");

        // Buffer should be empty after draining
        assert!(!output_state.has_buffered_output());

        // Cleanup
        output_state.cleanup();
    }

    fn make_test_session_id(label: &str) -> SessionId {
        format!(
            "test-{}-{}",
            label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
        .into()
    }

    #[test]
    fn test_bounded_buffer_retains_head_and_tail() {
        let session_id = make_test_session_id("bounded-head-tail");
        let mut output_state = OutputState::new(&session_id, 4);

        for i in 1..=10 {
            output_state
                .add_output(
                    &format!("Event {}\n", i),
                    Some(&OutputCategory::Stdout),
                    Seq(i as i64),
                )
                .unwrap();
        }

        assert_eq!(output_state.buffer_len(), 10);

        let buffered = output_state.take_buffered_output();
        assert_eq!(buffered.total_count, 10);
        assert_eq!(buffered.head.len(), 2);
        assert_eq!(buffered.tail.len(), 2);
        assert_eq!(buffered.head[0].output, "Event 1\n");
        assert_eq!(buffered.head[1].output, "Event 2\n");
        assert_eq!(buffered.tail[0].output, "Event 9\n");
        assert_eq!(buffered.tail[1].output, "Event 10\n");

        let content = output_state.read_last_lines(100).unwrap();
        for i in 1..=10 {
            assert!(
                content.contains(&format!("Event {}", i)),
                "Disk file should contain Event {}",
                i
            );
        }

        output_state.cleanup();
    }

    #[test]
    fn test_bounded_buffer_no_truncation_within_limit() {
        let session_id = make_test_session_id("bounded-within-limit");
        let mut output_state = OutputState::new(&session_id, 10);

        for i in 1..=5 {
            output_state
                .add_output(
                    &format!("Event {}\n", i),
                    Some(&OutputCategory::Stdout),
                    Seq(i as i64),
                )
                .unwrap();
        }

        assert_eq!(output_state.buffer_len(), 5);

        let buffered = output_state.take_buffered_output();
        assert_eq!(buffered.total_count, 5);
        let all_events: Vec<_> = buffered.head.iter().chain(buffered.tail.iter()).collect();
        assert_eq!(all_events.len(), 5);
        for (i, event) in all_events.iter().enumerate() {
            assert_eq!(event.output, format!("Event {}\n", i + 1));
        }

        output_state.cleanup();
    }

    #[test]
    fn test_bounded_buffer_resets_after_drain() {
        let session_id = make_test_session_id("bounded-reset");
        let mut output_state = OutputState::new(&session_id, 4);

        for i in 1..=10 {
            output_state
                .add_output(
                    &format!("Event {}\n", i),
                    Some(&OutputCategory::Stdout),
                    Seq(i as i64),
                )
                .unwrap();
        }

        let buffered = output_state.take_buffered_output();
        assert_eq!(buffered.total_count, 10);
        assert_eq!(buffered.head.len(), 2);
        assert_eq!(buffered.tail.len(), 2);
        assert_eq!(buffered.head[0].output, "Event 1\n");
        assert_eq!(buffered.head[1].output, "Event 2\n");
        assert_eq!(buffered.tail[0].output, "Event 9\n");
        assert_eq!(buffered.tail[1].output, "Event 10\n");

        assert_eq!(output_state.buffer_len(), 0);
        assert!(!output_state.has_buffered_output());

        for i in 11..=13 {
            output_state
                .add_output(
                    &format!("Event {}\n", i),
                    Some(&OutputCategory::Stdout),
                    Seq(i as i64),
                )
                .unwrap();
        }

        assert_eq!(output_state.buffer_len(), 3);

        let buffered = output_state.take_buffered_output();
        assert_eq!(buffered.total_count, 3);
        let all_events: Vec<_> = buffered.head.iter().chain(buffered.tail.iter()).collect();
        assert_eq!(all_events.len(), 3);
        assert_eq!(all_events[0].output, "Event 11\n");
        assert_eq!(all_events[1].output, "Event 12\n");
        assert_eq!(all_events[2].output, "Event 13\n");

        output_state.cleanup();
    }

    #[test]
    fn test_bounded_buffer_size_one_keeps_last_event() {
        let session_id = make_test_session_id("bounded-size-one");
        let mut output_state = OutputState::new(&session_id, 1);

        for i in 1..=5 {
            output_state
                .add_output(
                    &format!("Event {}\n", i),
                    Some(&OutputCategory::Stdout),
                    Seq(i as i64),
                )
                .unwrap();
        }

        let buffered = output_state.take_buffered_output();
        assert_eq!(buffered.total_count, 5);
        assert!(buffered.head.is_empty());
        assert_eq!(buffered.tail.len(), 1);
        assert_eq!(buffered.tail[0].output, "Event 5\n");

        output_state.cleanup();
    }

    #[test]
    fn test_bounded_buffer_exactly_at_capacity() {
        let session_id = make_test_session_id("bounded-exact-cap");
        let mut output_state = OutputState::new(&session_id, 4);

        for i in 1..=4 {
            output_state
                .add_output(
                    &format!("Event {}\n", i),
                    Some(&OutputCategory::Stdout),
                    Seq(i as i64),
                )
                .unwrap();
        }

        let buffered = output_state.take_buffered_output();
        assert_eq!(buffered.total_count, 4);
        assert_eq!(buffered.head.len(), 2);
        assert_eq!(buffered.tail.len(), 2);
        assert_eq!(buffered.head[0].output, "Event 1\n");
        assert_eq!(buffered.head[1].output, "Event 2\n");
        assert_eq!(buffered.tail[0].output, "Event 3\n");
        assert_eq!(buffered.tail[1].output, "Event 4\n");

        output_state.cleanup();
    }
}
