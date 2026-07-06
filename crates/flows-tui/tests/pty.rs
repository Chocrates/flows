//! Real-terminal parity test: spawn the TUI demo binary under a PTY, render its
//! output with a vt100 emulator, drive a mouse drag, and assert the selection
//! highlight actually paints on screen. The Rust equivalent of the Python
//! pyte harness — validates the full stack (terminal, crossterm mouse decode,
//! App handlers, ratatui render) end-to-end.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize};

const DEMO_BIN: &str = env!("CARGO_BIN_EXE_tui_demo");

/// SGR mouse sequence (1006). 1-based coords, matching real terminals.
fn sgr(button: u16, col: u16, row: u16, press: bool) -> String {
    format!(
        "\x1b[<{button};{col};{row}{}",
        if press { 'M' } else { 'm' }
    )
}

#[test]
fn tui_selection_highlight_paints_in_real_terminal() {
    let pty = portable_pty::native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let cmd = CommandBuilder::new(DEMO_BIN);
    let mut child = pair.slave.spawn_command(cmd).expect("spawn tui_demo");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");

    let mut parser = vt100::Parser::new(24, 100, 0);
    let mut buf = [0u8; 8192];

    // Pump until the known demo text appears (or time out).
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut found_row: Option<u16> = None;
    while Instant::now() < deadline && found_row.is_none() {
        if let Ok(n) = reader.read(&mut buf) {
            if n > 0 {
                parser.process(&buf[..n]);
            }
        }
        let screen = parser.screen();
        for row in 0..24u16 {
            let line: String = (0..100)
                .map(|c| {
                    screen
                        .cell(row, c)
                        .map(|x| x.contents())
                        .unwrap_or_default()
                })
                .collect();
            if line.contains("SELECTABLE-TEXT") {
                found_row = Some(row);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }

    let row = found_row.expect("demo text never rendered");
    // Find the starting column of "SELECTABLE".
    let screen = parser.screen();
    let line: Vec<char> = (0..100)
        .map(|c| {
            screen
                .cell(row, c)
                .and_then(|x| x.contents().chars().next())
                .unwrap_or(' ')
        })
        .collect();
    let start_col = line.iter().collect::<String>().find("SELECTABLE").unwrap() as u16;

    // Drag across ~8 chars of the text (SGR coords are 1-based).
    let (c0, c1) = (start_col + 1, start_col + 8);
    let r = row + 1;
    write!(writer, "{}", sgr(0, c0, r, true)).unwrap(); // mouse down
    for c in (c0 + 1)..=c1 {
        write!(writer, "{}", sgr(32, c, r, true)).unwrap(); // drag
    }
    write!(writer, "{}", sgr(0, c1, r, false)).unwrap(); // up
    writer.flush().unwrap();

    // Pump to let the drag re-render.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut highlighted = 0;
    while Instant::now() < deadline {
        if let Ok(n) = reader.read(&mut buf) {
            if n > 0 {
                parser.process(&buf[..n]);
            }
        }
        let screen = parser.screen();
        highlighted = (start_col..start_col + 8)
            .filter(|&c| {
                matches!(
                    screen.cell(row, c).map(|x| x.bgcolor()),
                    Some(vt100::Color::Rgb(9, 68, 114)) // the selection highlight color
                )
            })
            .count();
        if highlighted > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }

    // Quit the TUI and reap.
    let _ = write!(writer, "q");
    let _ = writer.flush();
    let _ = child.wait();

    assert!(
        highlighted >= 4,
        "expected the selection highlight (#094472) to paint on the dragged cells, got {highlighted}"
    );
}
