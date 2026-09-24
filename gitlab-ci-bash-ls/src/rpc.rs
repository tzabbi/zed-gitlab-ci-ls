//! Minimal LSP base-protocol framing (`Content-Length` headers + JSON body).

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// Reads the next message. Returns `Ok(None)` when the stream ends.
/// Messages with an invalid JSON body are logged and skipped.
pub fn read(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    loop {
        let mut length = None;
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                return Ok(None);
            }
            let header = line.trim_end();
            if header.is_empty() {
                if length.is_some() {
                    break;
                }
                continue;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.trim().eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse::<usize>().ok();
            }
        }

        let mut body = vec![0; length.unwrap_or_default()];
        reader.read_exact(&mut body)?;
        match serde_json::from_slice(&body) {
            Ok(message) => return Ok(Some(message)),
            Err(error) => eprintln!("gitlab-ci-bash-ls: ignoring malformed message: {error}"),
        }
    }
}

pub fn write(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_messages() {
        let mut buffer = Vec::new();
        write(&mut buffer, &json!({"jsonrpc": "2.0", "method": "ä"})).unwrap();
        write(&mut buffer, &json!({"id": 1})).unwrap();
        let mut reader = io::Cursor::new(buffer);
        assert_eq!(
            read(&mut reader).unwrap(),
            Some(json!({"jsonrpc": "2.0", "method": "ä"}))
        );
        assert_eq!(read(&mut reader).unwrap(), Some(json!({"id": 1})));
        assert_eq!(read(&mut reader).unwrap(), None);
    }

    #[test]
    fn skips_malformed_bodies() {
        let input = b"Content-Length: 3\r\n\r\nnopContent-Length: 2\r\n\r\n{}";
        let mut reader = io::Cursor::new(&input[..]);
        assert_eq!(read(&mut reader).unwrap(), Some(json!({})));
    }
}
