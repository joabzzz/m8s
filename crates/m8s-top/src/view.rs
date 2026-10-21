//! Pure rendering: usable with an in-memory backend, never initializes a TTY.
use crate::{StatusSnapshot, client::ReadError, display_text};
use m8s_core::Phase;
use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

#[derive(Default)]
pub struct Dashboard {
    pub snapshot: Option<StatusSnapshot>,
    pub error: Option<ReadError>,
    pub offset: usize,
}
impl Dashboard {
    pub fn update(&mut self, result: Result<StatusSnapshot, ReadError>) {
        match result {
            Ok(snapshot) => {
                self.snapshot = Some(snapshot);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        if area.width < 100 || area.height < 14 {
            let state = if self.error.is_some() {
                "STALE / UNKNOWN"
            } else {
                "Status unverified"
            };
            let compact = format!(
                "m8s top | READ-ONLY\n{state}\nReadiness: UNKNOWN\nLast observation: UNKNOWN\nResize to 100x14 for node table"
            );
            frame.render_widget(Paragraph::new(compact), area);
            return;
        }
        let regions = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(area);
        let banner = if let Some(error) = self.error {
            format!("STALE / UNKNOWN: {error}; retained rows are NOT current observations")
        } else {
            "Read-only status | reported phases only | readiness NOT verified".into()
        };
        frame.render_widget(
            Paragraph::new(banner)
                .style(Style::default().fg(Color::Yellow))
                .block(Block::default().title("m8s top").borders(Borders::ALL)),
            regions[0],
        );
        let Some(snapshot) = &self.snapshot else {
            frame.render_widget(
                Paragraph::new(
                    "No usable status snapshot. Fleet state unknown; not an empty fleet.",
                ),
                regions[2],
            );
            frame.render_widget(
                Paragraph::new("q / Esc / Ctrl-C quit | r refresh | read-only"),
                regions[3],
            );
            return;
        };
        let pools = snapshot
            .pools
            .iter()
            .map(|(pool, desired)| format!("{} desired={desired}", display_text(pool)))
            .collect::<Vec<_>>()
            .join(" | ");
        let summary = format!(
            "mode={} | bootstrap=unverified | heartbeat=unknown | cost=unknown\n{}",
            display_text(&snapshot.reported_mode),
            if pools.is_empty() {
                "pools unknown"
            } else {
                &pools
            }
        );
        frame.render_widget(
            Paragraph::new(summary).block(Block::default().borders(Borders::ALL)),
            regions[1],
        );
        let height = usize::from(regions[2].height.saturating_sub(3));
        let offset = self.offset.min(snapshot.nodes.len().saturating_sub(1));
        let rows = snapshot.nodes.iter().skip(offset).take(height).map(|node| {
            let phase = match node.reported_phase {
                Phase::Ready => "reported Ready (unverified)".into(),
                phase => format!("{phase:?}"),
            };
            let teardown = match node.reported_phase {
                Phase::Destroying | Phase::Draining => "PENDING / unconfirmed",
                Phase::Orphaned => "orphan / unknown",
                Phase::Gone => "Gone reported only",
                _ => "unknown",
            };
            Row::new(vec![
                Cell::from(display_text(&node.uid)),
                Cell::from(display_text(&node.pool)),
                Cell::from(phase),
                Cell::from(
                    node.instance_id
                        .as_deref()
                        .map(display_text)
                        .unwrap_or_else(|| "unknown".into()),
                ),
                Cell::from("unknown"),
                Cell::from("unknown"),
                Cell::from(teardown),
            ])
            .style(Style::default().fg(if node.destroy_pending == Some(true) {
                Color::LightRed
            } else {
                Color::White
            }))
        });
        let widths = [
            Constraint::Percentage(12),
            Constraint::Percentage(8),
            Constraint::Length(27),
            Constraint::Percentage(10),
            Constraint::Percentage(8),
            Constraint::Percentage(8),
            Constraint::Percentage(19),
        ];
        let table = Table::new(rows, widths)
            .header(
                Row::new([
                    "NODE UID",
                    "POOL",
                    "PHASE (API)",
                    "INSTANCE",
                    "LAST OBS",
                    "MODEL READY",
                    "TEARDOWN / ABSENCE",
                ])
                .style(Style::default().fg(Color::Cyan)),
            )
            .column_spacing(1)
            .block(Block::default().borders(Borders::ALL).title("Nodes"));
        frame.render_widget(table, regions[2]);
        if snapshot.nodes.is_empty() {
            // A successful empty list is only a reported snapshot, not proof all
            // historical provider resources or residual storage are gone.
            let inner = ratatui::layout::Rect::new(
                regions[2].x + 1,
                regions[2].y + 2,
                regions[2].width.saturating_sub(2),
                regions[2].height.saturating_sub(3),
            );
            frame.render_widget(
                Paragraph::new("No nodes reported; provider absence not independently verified."),
                inner,
            );
        }
        let footer = format!(
            "q/Esc/Ctrl-C quit | r refresh | j/k scroll | rows {}..{} of {}\nObservation/model/canary evidence unavailable in status v1; unknown is not zero or Ready.",
            if snapshot.nodes.is_empty() {
                0
            } else {
                offset + 1
            },
            (offset + height).min(snapshot.nodes.len()),
            snapshot.nodes.len()
        );
        frame.render_widget(
            Paragraph::new(footer).block(Block::default().borders(Borders::TOP)),
            regions[3],
        );
    }
}

/// Bound terminal-reported dimensions before allocating renderer buffers.
pub fn bounded_area(width: u16, height: u16) -> ratatui::layout::Rect {
    ratatui::layout::Rect::new(0, 0, width.min(300), height.min(120))
}

pub fn text_snapshot(dashboard: &Dashboard, width: u16, height: u16) -> std::io::Result<String> {
    if width == 0 || height == 0 || width > 300 || height > 120 {
        return Err(std::io::Error::other("invalid snapshot dimensions"));
    }
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    terminal.draw(|frame| dashboard.render(frame))?;
    let buffer = terminal.backend().buffer();
    let mut output = String::new();
    for y in 0..height {
        let mut line = String::new();
        for x in 0..width {
            line.push_str(buffer[(x, y)].symbol());
        }
        output.push_str(line.trim_end());
        output.push('\n');
    }
    Ok(output)
}
