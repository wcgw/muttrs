//! Messages as Unix tools take them: one as it is, with Unix line endings; several as an mbox (mboxrd), as `git am`
//! and `b4` read a series of patches.

/// `messages`, as RFC 5322 writes each, written for a command's input: one as it is, several each after a `From `
/// line, `From ` lines of their own quoted (`>From `, and `>>From ` for `>From `, as mboxrd does); with Unix line
/// endings.
#[must_use]
pub fn for_commands(messages: &[&[u8]]) -> Vec<u8> {
    if let [message] = messages {
        return unix_lines(message);
    }
    let mut mbox = Vec::new();
    for message in messages {
        mbox.extend_from_slice(b"From muttrs Thu Jan  1 00:00:00 1970\n");
        let lines = unix_lines(message);
        for line in lines.split_inclusive(|&byte| byte == b'\n') {
            let quoted = line.iter().position(|&byte| byte != b'>');
            if quoted.is_some_and(|at| line[at..].starts_with(b"From ")) {
                mbox.push(b'>');
            }
            mbox.extend_from_slice(line);
        }
        if !mbox.ends_with(b"\n") {
            mbox.push(b'\n');
        }
        // A blank line before the next `From ` line.
        mbox.push(b'\n');
    }
    mbox
}

/// `bytes` with CRLF line endings made LF.
fn unix_lines(bytes: &[u8]) -> Vec<u8> {
    let mut lines = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some(at) = rest.windows(2).position(|pair| pair == b"\r\n") {
        lines.extend_from_slice(&rest[..at]);
        lines.push(b'\n');
        rest = &rest[at + 2..];
    }
    lines.extend_from_slice(rest);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_message_is_piped_as_it_is_with_unix_line_endings() {
        assert_eq!(
            for_commands(&[b"Subject: a\r\n\r\nFrom here\r\nbody\r\n"]),
            b"Subject: a\n\nFrom here\nbody\n"
        );
    }

    #[test]
    fn several_are_an_mboxrd_their_from_lines_quoted() {
        let one = b"Subject: [PATCH 1/2] a\r\n\r\nFrom here\r\n>From there\r\n";
        let two = b"Subject: [PATCH 2/2] b\r\n\r\nno newline at the end";
        assert_eq!(
            String::from_utf8(for_commands(&[one, two])).unwrap(),
            "From muttrs Thu Jan  1 00:00:00 1970\n\
             Subject: [PATCH 1/2] a\n\n>From here\n>>From there\n\n\
             From muttrs Thu Jan  1 00:00:00 1970\n\
             Subject: [PATCH 2/2] b\n\nno newline at the end\n\n"
        );
    }
}
