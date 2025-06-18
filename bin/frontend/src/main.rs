use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::rc::Rc;

use ratzilla::ratatui::{
    Terminal,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph},
};
use ratzilla::{DomBackend, WebRenderer, event::KeyCode};
use wasm_bindgen::prelude::*;
use web_sys::{MessageEvent, WebSocket};

// Maximum number of messages to keep before removing oldest
const MAX_MESSAGES: usize = 256;

const TITLE: &str = " App ";

fn main() -> io::Result<()> {
    // Shared state
    let messages = Rc::new(RefCell::new(VecDeque::with_capacity(MAX_MESSAGES)));
    let input_buffer = Rc::new(RefCell::new(String::new()));
    let backend = DomBackend::new()?;
    let terminal = Terminal::new(backend)?;

    // Setup WebSocket
    let ws = Rc::new(RefCell::new(setup_websocket(messages.clone())));

    // Handle keyboard input
    terminal.on_key_event({
        let messages = messages.clone();
        let input_buffer = input_buffer.clone();
        let ws = ws.clone();

        move |key_event| {
            match key_event.code {
                KeyCode::Enter => {
                    // Send message when Enter is pressed
                    let msg = input_buffer.borrow().clone();
                    if !msg.is_empty() {
                        if let Err(e) = ws.borrow().send_with_str(&msg) {
                            add_message(&messages, format!("Send error: {:?}", e));
                        }
                        input_buffer.borrow_mut().clear();
                    }
                }
                KeyCode::Backspace => {
                    // Handle backspace
                    input_buffer.borrow_mut().pop();
                }
                KeyCode::Char(c) => {
                    // Add character to input buffer
                    input_buffer.borrow_mut().push(c);
                }
                _ => {}
            }
        }
    });

    // Render loop
    terminal.draw_web(move |f| {
        // Create outer border
        let outer_block = Block::default()
            .title(TITLE)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightCyan));

        let mut area = f.area();
        area.x += 1;
        area.width -= 1;
        f.render_widget(outer_block, area);

        // Calculate inner area (inside the border)
        let inner_area = {
            let mut area = area;
            area.x += 1;
            area.y += 1;
            area.width = area.width.saturating_sub(2);
            area.height = area.height.saturating_sub(2);
            area
        };

        // Split the inner area into message area and input area
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                [
                    Constraint::Min(1),    // Message area (expands to fill space)
                    Constraint::Length(3), // Fixed height input area
                ]
                .as_ref(),
            )
            .split(inner_area);

        // Render messages in the upper area
        let msgs = messages.borrow();
        let rows = chunks[0].height as usize;

        // Create styled text from ANSI codes
        let mut text = Text::default();
        for msg in msgs.iter().skip(msgs.len().saturating_sub(rows)) {
            let line = parse_ansi_to_line(msg);
            text.lines.push(line);
        }

        f.render_widget(
            Paragraph::new(text).block(Block::default().borders(Borders::NONE)),
            chunks[0],
        );

        // Render input in the lower area
        let input = input_buffer.borrow();
        f.render_widget(
            Paragraph::new(format!("> {}", input))
                .block(
                    Block::default()
                        .title(" Input ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::LightMagenta)),
                )
                .alignment(Alignment::Left),
            chunks[1],
        );
    });

    Ok(())
}

// Parse ANSI color codes to Ratatui spans
fn parse_ansi_to_line(input: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut current_style = Style::default();
    let mut buffer = String::new();

    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Handle ANSI escape sequence
            if chars.next() == Some('[') {
                let mut code = String::new();
                while let Some(&c) = chars.peek() {
                    if c == 'm' {
                        chars.next(); // consume 'm'
                        break;
                    }
                    code.push(chars.next().unwrap());
                }

                // Push the buffered text with current style
                if !buffer.is_empty() {
                    spans.push(Span::styled(buffer.clone(), current_style));
                    buffer.clear();
                }

                // Update style based on ANSI code
                current_style = match code.as_str() {
                    "31" => Style::default().fg(Color::Red),
                    "32" => Style::default().fg(Color::Green),
                    "33" => Style::default().fg(Color::Yellow),
                    "34" => Style::default().fg(Color::Blue),
                    "35" => Style::default().fg(Color::Magenta),
                    "36" => Style::default().fg(Color::Cyan),
                    "37" => Style::default().fg(Color::White),
                    "39" => Style::default(), // reset
                    "90" => Style::default().fg(Color::Gray),
                    _ => current_style,
                };
            }
        } else {
            buffer.push(c);
        }
    }

    // Push any remaining text
    if !buffer.is_empty() {
        spans.push(Span::styled(buffer, current_style));
    }

    Line::from(spans)
}

// Helper function to add messages with automatic pruning
fn add_message(messages: &Rc<RefCell<VecDeque<String>>>, message: String) {
    let mut msgs = messages.borrow_mut();
    msgs.push_back(message);

    // Remove oldest messages if we exceed the maximum
    if msgs.len() > MAX_MESSAGES {
        msgs.remove(0);
    }
}

fn setup_websocket(messages: Rc<RefCell<VecDeque<String>>>) -> WebSocket {
    let ws = WebSocket::new("/ws").unwrap();

    let onmessage_callback = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        if let Some(text) = e.data().as_string() {
            add_message(&messages, text);
        }
    });
    ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
    onmessage_callback.forget();

    ws
}
