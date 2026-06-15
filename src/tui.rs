use std::collections::HashSet;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::Stage;
use crate::runner::monitor::PipelineMonitor;

use crossterm::{
    event::KeyCode,
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Terminal,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone)]
pub struct StepState {
    pub name: String,
    pub image: String,
    pub status: Status,
    pub logs: Vec<(String, bool)>, // (log_text, is_stderr)
    pub start_time: Option<Instant>,
    pub duration: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct StageState {
    pub name: String,
    pub parallel: bool,
    pub status: Status,
    pub steps: Vec<StepState>,
    pub start_time: Option<Instant>,
    pub duration: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePanel {
    StagesList,
    LogsBlock,
}

pub struct TuiState {
    pub stages: Vec<StageState>,
    pub pipeline_start: Instant,
    pub pipeline_duration: Option<Duration>,
    pub pipeline_success: Option<bool>,
    pub active_containers: HashSet<String>,
    
    // UI states
    pub selected_step_idx: usize,
    pub active_panel: ActivePanel,
    pub log_scroll: usize,
    pub auto_scroll: bool,
    pub auto_focus: bool,
    pub should_quit: bool,
    pub error_msg: Option<String>,
}

impl TuiState {
    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
            pipeline_start: Instant::now(),
            pipeline_duration: None,
            pipeline_success: None,
            active_containers: HashSet::new(),
            selected_step_idx: 0,
            active_panel: ActivePanel::StagesList,
            log_scroll: 0,
            auto_scroll: true,
            auto_focus: true,
            should_quit: false,
            error_msg: None,
        }
    }

    pub fn get_flat_steps(&self) -> Vec<(usize, usize)> {
        let mut list = Vec::new();
        for (stage_idx, stage) in self.stages.iter().enumerate() {
            for (step_idx, _) in stage.steps.iter().enumerate() {
                list.push((stage_idx, step_idx));
            }
        }
        list
    }

    pub fn get_selected_step(&self) -> Option<&StepState> {
        let flat = self.get_flat_steps();
        if flat.is_empty() || self.selected_step_idx >= flat.len() {
            None
        } else {
            let (stage_idx, step_idx) = flat[self.selected_step_idx];
            Some(&self.stages[stage_idx].steps[step_idx])
        }
    }
}

pub struct TuiMonitor {
    pub state: Arc<Mutex<TuiState>>,
}

impl TuiMonitor {
    pub fn new(state: Arc<Mutex<TuiState>>) -> Self {
        Self { state }
    }
}

impl PipelineMonitor for TuiMonitor {
    fn on_pipeline_start(&self, stages: &[Stage]) {
        let mut state = self.state.lock().unwrap();
        state.stages = stages
            .iter()
            .map(|s| StageState {
                name: s.name.clone(),
                parallel: s.parallel,
                status: Status::Pending,
                steps: s
                    .steps
                    .iter()
                    .map(|st| StepState {
                        name: st.name.clone(),
                        image: st.image.clone(),
                        status: Status::Pending,
                        logs: Vec::new(),
                        start_time: None,
                        duration: None,
                    })
                    .collect(),
                start_time: None,
                duration: None,
            })
            .collect();
    }

    fn on_stage_start(&self, stage_name: &str, _parallel: bool) {
        let mut state = self.state.lock().unwrap();
        if let Some(stage) = state.stages.iter_mut().find(|s| s.name == stage_name) {
            stage.status = Status::Running;
            stage.start_time = Some(Instant::now());
        }
    }

    fn on_stage_complete(&self, stage_name: &str, success: bool) {
        let mut state = self.state.lock().unwrap();
        if let Some(stage) = state.stages.iter_mut().find(|s| s.name == stage_name) {
            stage.status = if success { Status::Succeeded } else { Status::Failed };
            if let Some(start) = stage.start_time {
                stage.duration = Some(start.elapsed());
            }
        }
    }

    fn on_step_start(&self, step_name: &str, _image: &str) {
        let mut state = self.state.lock().unwrap();
        let mut found_indices = None;
        for (stage_idx, stage) in state.stages.iter_mut().enumerate() {
            for (step_idx, step) in stage.steps.iter_mut().enumerate() {
                if step.name == step_name {
                    step.status = Status::Running;
                    step.start_time = Some(Instant::now());
                    found_indices = Some((stage_idx, step_idx));
                }
            }
        }

        // Auto focus if enabled
        if let Some((stage_idx, step_idx)) = found_indices {
            if state.auto_focus {
                let flat = state.get_flat_steps();
                if let Some(pos) = flat.iter().position(|&(st_i, sp_i)| st_i == stage_idx && sp_i == step_idx) {
                    state.selected_step_idx = pos;
                }
            }
        }
    }

    fn on_step_log(&self, step_name: &str, log_line: &str, is_stderr: bool) {
        let mut state = self.state.lock().unwrap();
        for stage in &mut state.stages {
            for step in &mut stage.steps {
                if step.name == step_name {
                    step.logs.push((log_line.to_string(), is_stderr));
                }
            }
        }
    }

    fn on_step_complete(&self, step_name: &str, success: bool) {
        let mut state = self.state.lock().unwrap();
        for stage in &mut state.stages {
            for step in &mut stage.steps {
                if step.name == step_name {
                    step.status = if success { Status::Succeeded } else { Status::Failed };
                    if let Some(start) = step.start_time {
                        step.duration = Some(start.elapsed());
                    }
                }
            }
        }
    }

    fn on_pipeline_complete(&self, success: bool, duration: Duration) {
        let mut state = self.state.lock().unwrap();
        state.pipeline_success = Some(success);
        state.pipeline_duration = Some(duration);
    }

    fn on_container_created(&self, container_id: &str) {
        let mut state = self.state.lock().unwrap();
        state.active_containers.insert(container_id.to_string());
    }

    fn on_container_destroyed(&self, container_id: &str) {
        let mut state = self.state.lock().unwrap();
        state.active_containers.remove(container_id);
    }
}

pub async fn run_tui(state: Arc<Mutex<TuiState>>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Use drop guard to recover terminal even on panic
    let _guard = CleanTerminal;

    // We'll create channels for event polling
    enum TuiEvent {
        Key(crossterm::event::KeyEvent),
        Resize,
        Tick,
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Event poll thread
    let rt = tokio::runtime::Handle::current();
    let event_tx = tx.clone();
    std::thread::spawn(move || {
        loop {
            if let Ok(event) = crossterm::event::read() {
                let tui_event = match event {
                    crossterm::event::Event::Key(key) => TuiEvent::Key(key),
                    crossterm::event::Event::Resize(_, _) => TuiEvent::Resize,
                    _ => continue,
                };
                if rt.block_on(event_tx.send(tui_event)).is_err() {
                    break;
                }
            }
        }
    });

    // Tick thread
    let tick_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(50));
        loop {
            interval.tick().await;
            if tick_tx.send(TuiEvent::Tick).await.is_err() {
                break;
            }
        }
    });

    loop {
        // Redraw
        terminal.draw(|f| draw_ui(f, &state))?;

        // Read event
        match rx.recv().await {
            Some(TuiEvent::Key(key)) => {
                let mut s = state.lock().unwrap();
                
                // General keys
                match key.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => {
                        s.should_quit = true;
                        break;
                    }
                    KeyCode::Tab => {
                        s.active_panel = match s.active_panel {
                            ActivePanel::StagesList => ActivePanel::LogsBlock,
                            ActivePanel::LogsBlock => ActivePanel::StagesList,
                        };
                    }
                    KeyCode::Char('a') | KeyCode::Char('A') => {
                        s.auto_scroll = !s.auto_scroll;
                        if s.auto_scroll {
                            s.auto_focus = true;
                        }
                    }
                    KeyCode::Up => {
                        match s.active_panel {
                            ActivePanel::StagesList => {
                                let flat = s.get_flat_steps();
                                if !flat.is_empty() {
                                    s.auto_focus = false;
                                    if s.selected_step_idx > 0 {
                                        s.selected_step_idx -= 1;
                                    } else {
                                        s.selected_step_idx = flat.len() - 1;
                                    }
                                    s.log_scroll = 0;
                                    s.auto_scroll = true;
                                }
                            }
                            ActivePanel::LogsBlock => {
                                s.auto_scroll = false;
                                if s.log_scroll > 0 {
                                    s.log_scroll -= 1;
                                }
                            }
                        }
                    }
                    KeyCode::Down => {
                        match s.active_panel {
                            ActivePanel::StagesList => {
                                let flat = s.get_flat_steps();
                                if !flat.is_empty() {
                                    s.auto_focus = false;
                                    if s.selected_step_idx < flat.len() - 1 {
                                        s.selected_step_idx += 1;
                                    } else {
                                        s.selected_step_idx = 0;
                                    }
                                    s.log_scroll = 0;
                                    s.auto_scroll = true;
                                }
                            }
                            ActivePanel::LogsBlock => {
                                s.auto_scroll = false;
                                s.log_scroll += 1;
                            }
                        }
                    }
                    KeyCode::PageUp => {
                        if s.active_panel == ActivePanel::LogsBlock {
                            s.auto_scroll = false;
                            if s.log_scroll >= 10 {
                                s.log_scroll -= 10;
                            } else {
                                s.log_scroll = 0;
                            }
                        }
                    }
                    KeyCode::PageDown => {
                        if s.active_panel == ActivePanel::LogsBlock {
                            s.auto_scroll = false;
                            s.log_scroll += 10;
                        }
                    }
                    _ => {}
                }
            }
            Some(TuiEvent::Resize) => {
                let _ = terminal.clear();
            }
            Some(TuiEvent::Tick) => {
                let s = state.lock().unwrap();
                if s.should_quit {
                    break;
                }
            }
            None => break,
        }
    }

    Ok(())
}

fn draw_ui(f: &mut ratatui::Frame, state: &Arc<Mutex<TuiState>>) {
    let mut s = state.lock().unwrap();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(5),    // Main Panels
            Constraint::Length(3), // Footer
        ])
        .split(f.area());

    // --- 1. Draw Header ---
    let elapsed = if let Some(dur) = s.pipeline_duration {
        dur
    } else {
        s.pipeline_start.elapsed()
    };
    
    // Count stats
    let mut total_steps = 0;
    let mut completed_steps = 0;
    let mut failed_steps = 0;
    for stage in &s.stages {
        for step in &stage.steps {
            total_steps += 1;
            match step.status {
                Status::Succeeded => completed_steps += 1,
                Status::Failed => failed_steps += 1,
                _ => {}
            }
        }
    }

    let status_str = match s.pipeline_success {
        None => "RUNNING".cyan().bold(),
        Some(true) => "SUCCESS".green().bold(),
        Some(false) => "FAILED".red().bold(),
    };

    let mut stats_text = vec![
        Span::raw(" FORGE LOCAL CI/CD  |  Status: "),
        status_str,
        Span::raw(format!(
            "  |  Duration: {:02}:{:02}s  |  Steps: {}/{} Succeeded",
            elapsed.as_secs() / 60,
            elapsed.as_secs() % 60,
            completed_steps,
            total_steps
        )),
    ];
    
    if failed_steps > 0 {
        stats_text.push(Span::raw(" | "));
        stats_text.push(format!("{} Failed", failed_steps).red().bold());
    }

    let header_p = Paragraph::new(Line::from(stats_text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Rgb(99, 102, 241))),
        );
    f.render_widget(header_p, chunks[0]);

    // --- 2. Draw Main Area (Split Left/Right) ---
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40), // Stages & Steps
            Constraint::Percentage(60), // Logs
        ])
        .split(chunks[1]);

    // Left Border Highlight
    let stages_border_color = if s.active_panel == ActivePanel::StagesList {
        Color::Rgb(99, 102, 241)
    } else {
        Color::DarkGray
    };
    let stages_block = Block::default()
        .title(" Stages & Steps ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(stages_border_color));

    let mut list_items = Vec::new();
    let flat_steps = s.get_flat_steps();

    for (stage_idx, stage) in s.stages.iter().enumerate() {
        let stage_icon = match stage.status {
            Status::Pending => Span::raw("─").dim(),
            Status::Running => Span::raw("▶").cyan().bold(),
            Status::Succeeded => Span::raw("✔").green(),
            Status::Failed => Span::raw("✘").red().bold(),
        };
        
        let parallel_str = if stage.parallel { " [parallel]" } else { "" };
        let stage_line = Line::from(vec![
            stage_icon,
            Span::raw(" Stage: "),
            Span::raw(stage.name.clone()).bold(),
            Span::raw(parallel_str).yellow().dim(),
        ]);
        list_items.push(ListItem::new(stage_line));

        for (step_idx, step) in stage.steps.iter().enumerate() {
            let is_selected = flat_steps
                .iter()
                .position(|&(st_i, sp_i)| st_i == stage_idx && sp_i == step_idx)
                == Some(s.selected_step_idx);

            let step_icon = match step.status {
                Status::Pending => Span::raw("─").dim(),
                Status::Running => Span::raw("▶").cyan().bold(),
                Status::Succeeded => Span::raw("✔").green(),
                Status::Failed => Span::raw("✘").red().bold(),
            };

            let prefix = if is_selected { "  ├─ > " } else { "  ├─   " };
            
            let mut step_spans = vec![
                Span::raw(prefix),
                step_icon,
                Span::raw(" "),
                Span::raw(step.name.clone()),
            ];

            if let Some(dur) = step.duration {
                step_spans.push(Span::raw(format!(" ({:.1}s)", dur.as_secs_f64())).dim());
            }

            let step_line = Line::from(step_spans);
            let mut item = ListItem::new(step_line);
            if is_selected {
                item = item.style(Style::default().bg(Color::Rgb(30, 41, 59)));
            }
            list_items.push(item);
        }
    }

    let stages_list = List::new(list_items).block(stages_block);
    f.render_widget(stages_list, main_chunks[0]);

    // Right Border Highlight
    let logs_border_color = if s.active_panel == ActivePanel::LogsBlock {
        Color::Rgb(99, 102, 241)
    } else {
        Color::DarkGray
    };

    let selected_step = s.get_selected_step();
    let logs_title = match selected_step {
        Some(step) => format!(" Logs: {} ", step.name),
        None => " Logs ".to_string(),
    };

    let logs_block = Block::default()
        .title(logs_title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(logs_border_color));

    let mut log_lines = Vec::new();
    if let Some(step) = selected_step {
        for (chunk, is_err) in &step.logs {
            let mut parts: Vec<&str> = chunk.split('\n').collect();
            if parts.len() > 1 && parts.last().map_or(false, |p| p.is_empty()) {
                parts.pop();
            }
            for part in parts {
                let span = if *is_err {
                    Span::raw(part.to_string()).red()
                } else {
                    Span::raw(part.to_string())
                };
                log_lines.push(Line::from(span));
            }
        }
    }

    // Determine scrolling bounds & clamp
    let logs_height = (main_chunks[1].height as usize).saturating_sub(2);
    let mut scroll_offset = s.log_scroll;
    
    if s.auto_scroll {
        if log_lines.len() > logs_height {
            scroll_offset = log_lines.len() - logs_height;
        } else {
            scroll_offset = 0;
        }
        s.log_scroll = scroll_offset;
    } else {
        let max_scroll = log_lines.len().saturating_sub(logs_height);
        if scroll_offset > max_scroll {
            scroll_offset = max_scroll;
        }
        s.log_scroll = scroll_offset;
    }

    let visible_logs: Vec<Line> = log_lines
        .into_iter()
        .skip(scroll_offset)
        .take(logs_height)
        .collect();

    let logs_p = Paragraph::new(visible_logs)
        .block(logs_block)
        .wrap(Wrap { trim: false });
    f.render_widget(logs_p, main_chunks[1]);

    // --- 3. Draw Footer ---
    let auto_scroll_status = if s.auto_scroll { "ON".green() } else { "OFF".dark_gray() };
    let active_panel_str = match s.active_panel {
        ActivePanel::StagesList => "STAGES".cyan(),
        ActivePanel::LogsBlock => "LOGS".cyan(),
    };
    
    let footer_text = vec![
        Span::raw(" [q] Quit  |  [Tab] Switch Panel ("),
        active_panel_str,
        Span::raw(")  |  [↑/↓] Navigate  |  [a] Auto-Scroll ("),
        auto_scroll_status,
        Span::raw(")"),
    ];

    let footer_p = Paragraph::new(Line::from(footer_text))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    f.render_widget(footer_p, chunks[2]);
}

struct CleanTerminal;
impl Drop for CleanTerminal {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}
