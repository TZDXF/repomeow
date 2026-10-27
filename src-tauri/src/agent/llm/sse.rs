//! SSE 行分割与仅含 data 的解码器。

#[derive(Default)]
pub(super) struct DataSseDecoder {
    buffer: Vec<u8>,
    data_lines: Vec<String>,
    trim_all_cr: bool,
}

impl DataSseDecoder {
    // Responses 原实现移除行尾全部 CR；Google 的 Default 只移除一个。
    pub(super) fn new() -> Self {
        Self {
            trim_all_cr: true,
            ..Self::default()
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(position) = self.buffer.iter().position(|&byte| byte == b'\n') {
            let line_bytes: Vec<u8> = self.buffer.drain(..=position).collect();
            let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);
            let line = if self.trim_all_cr {
                line.trim_end_matches('\r')
            } else {
                line.strip_suffix('\r').unwrap_or(&line)
            };
            if line.is_empty() {
                if !self.data_lines.is_empty() {
                    events.push(std::mem::take(&mut self.data_lines).join("\n"));
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data_lines
                    .push(data.strip_prefix(' ').unwrap_or(data).to_string());
            }
        }
        events
    }

    // 仅 Responses 在 EOF 时调用；Google 保持不刷新末尾未完成帧。
    pub(super) fn finish(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line_bytes = std::mem::take(&mut self.buffer);
            let line = String::from_utf8_lossy(&line_bytes);
            let line = line.trim_end_matches('\r');
            if let Some(data) = line.strip_prefix("data:") {
                self.data_lines
                    .push(data.strip_prefix(' ').unwrap_or(data).to_string());
            }
        }
        if !self.data_lines.is_empty() {
            events.push(std::mem::take(&mut self.data_lines).join("\n"));
        }
        events
    }
}

pub(super) fn next_line(buffer: &[u8]) -> Option<(usize, usize)> {
    let carriage_return = buffer.iter().position(|&byte| byte == b'\r');
    let newline = buffer.iter().position(|&byte| byte == b'\n');
    let index = match (carriage_return, newline) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };
    let mut consumed = index + 1;
    if buffer[index] == b'\r' && buffer.get(consumed) == Some(&b'\n') {
        consumed += 1;
    }
    Some((index, consumed))
}

#[cfg(test)]
mod tests {
    use super::DataSseDecoder;

    #[test]
    fn cr_handling_remains_provider_specific() {
        let mut responses = DataSseDecoder::new();
        let mut google = DataSseDecoder::default();
        assert_eq!(responses.push(b"data: a\r\r\n\n"), vec!["a"]);
        assert_eq!(google.push(b"data: a\r\r\n\n"), vec!["a\r"]);
    }
}
