use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, BorderType, Chart, Dataset, Gauge, GraphType, LineGauge, Paragraph, Row,
    Sparkline, Table, TableState,
};
use ratatui::{DefaultTerminal, Frame};
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};
use sysinfo::{Disk, Disks, MINIMUM_CPU_UPDATE_INTERVAL, ProcessesToUpdate, System};

const TICK_RATE: Duration = Duration::from_millis(500);
const DARK_GRAY: Color = Color::DarkGray;
const TEXT_COLOR: Color = Color::Cyan;
const HISTORY_LEN: usize = 120;
const RAM_HISTORY: Color = Color::Magenta;

enum SortBy {
    Cpu,
    Memory,
}

struct ProcessRow {
    pid: u32,
    name: String,
    cpu: f32,
    memory_mb: u64,
}

struct Watcher {
    system: System,
    disks: Vec<Disk>,
    processes: Vec<ProcessRow>,
    process_state: TableState,
    process_sort: SortBy,
    cpu_history: VecDeque<f64>,
    ram_history: VecDeque<f64>,
    hostname: String,
    os_name: String,
    exit: bool,
}

impl Watcher {
    fn new() -> Self {
        let mut system = System::new_all().unwrap();
        std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
        system.refresh_cpu_usage();
        system.refresh_memory();
        let mut wather = Watcher {
            system,
            disks: load_disks(),
            processes: Vec::new(),
            process_state: TableState::default().with_selected(0),
            process_sort: SortBy::Cpu,
            cpu_history: VecDeque::with_capacity(HISTORY_LEN),
            ram_history: VecDeque::with_capacity(HISTORY_LEN),
            hostname: System::host_name().unwrap_or_else(|_| "unknown".into()),
            os_name: System::long_os_version().unwrap_or_else(|_| "unknown".into()),
            exit: false,
        };
        wather.on_tick();
        wather
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        let mut last_tick = Instant::now();

        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;

            let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());

            if event::poll(timeout)?
                && let Event::Key(key) = event::read()?
            {
                self.handle_key(key);
            }
            if last_tick.elapsed() >= TICK_RATE {
                self.on_tick();
                last_tick = Instant::now();
            }
        }

        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.exit = true,
            KeyCode::Char('j') => self.process_state.select_next(),
            KeyCode::Char('k') => self.process_state.select_previous(),
            KeyCode::Char('c') => {
                self.process_sort = SortBy::Cpu;
                self.sort_processes();
            }
            KeyCode::Char('m') => {
                self.process_sort = SortBy::Memory;
                self.sort_processes();
            }
            _ => {}
        }
    }

    fn on_tick(&mut self) {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        for disk in &mut self.disks {
            disk.refresh();
        }
        self.system.refresh_processes(ProcessesToUpdate::All, true);
        push_history(&mut self.cpu_history, self.system.global_cpu_usage() as f64);
        let ram = calc_percent(self.system.used_memory(), self.system.total_memory());
        push_history(&mut self.ram_history, ram);
        self.collect_processes();
    }

    fn collect_processes(&mut self) {
        let cores = self.system.cpus().len().max(1) as f32;
        self.processes = self
            .system
            .processes()
            .values()
            .map(|p| ProcessRow {
                pid: p.pid().as_u32(),
                name: p.name().to_string_lossy().to_string(),
                cpu: p.cpu_usage() / cores,
                memory_mb: p.memory() / 1024 / 1024,
            })
            .collect();
        self.sort_processes()
    }

    fn sort_processes(&mut self) {
        match self.process_sort {
            SortBy::Cpu => self.processes.sort_by(|a, b| b.cpu.total_cmp(&a.cpu)),
            SortBy::Memory => self
                .processes
                .sort_by_key(|a| std::cmp::Reverse(a.memory_mb)),
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let top_height = self.top_height();
        let [header, top, history, processes, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(top_height),
            Constraint::Length(10),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        let [cpu, memory] = Layout::horizontal([Constraint::Percentage(50); 2]).areas(top);

        self.render_header(frame, header);
        self.render_cpu(frame, cpu);
        self.render_memory(frame, memory);
        self.render_history(frame, history);
        self.render_processes(frame, processes);
        self.render_footer(frame, footer);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let sep = Span::styled(" | ", DARK_GRAY);
        let uptime = System::uptime();
        let line = Line::from(vec![
            "System Watcher".bold().fg(TEXT_COLOR),
            sep.clone(),
            self.hostname.clone().bold(),
            sep.clone(),
            self.os_name.clone().bold(),
            sep.clone(),
            Span::raw(if let Ok(up) = uptime {
                format!("Uptime {}h {}m", up / 3600, up % 3600 / 60)
            } else {
                "Somthing went wrong".to_string()
            }),
        ])
        .centered();
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(DARK_GRAY);
        frame.render_widget(Paragraph::new(line).block(block), area);
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let footer_info = |key: &str, desc: &str| {
            [
                Span::styled(format!(" {key} "), DARK_GRAY),
                Span::styled(format!(" {desc}\t "), DARK_GRAY),
            ]
        };

        let spans: Vec<Span<'_>> = [footer_info("q", "quit")].into_iter().flatten().collect();
        frame.render_widget(Line::from(spans), area);
    }

    fn render_cpu(&self, frame: &mut Frame, area: Rect) {
        let avg_cpu = self.system.global_cpu_usage();
        let block = style_block(&format!("CPU {avg_cpu:.0}%"));
        let inner_block = block.inner(area);
        frame.render_widget(block, area);
        let core_rows = self.system.cpus().len().div_ceil(2) as u16;
        let [core_area, spark_area] =
            Layout::vertical([Constraint::Length(core_rows), Constraint::Fill(1)])
                .spacing(2)
                .areas(inner_block);
        let [left, right] = Layout::horizontal([Constraint::Fill(1); 2])
            .spacing(2)
            .areas(core_area);

        for (i, cpu) in self.system.cpus().iter().enumerate() {
            let n = i + 1;
            let usage = cpu.usage() as f64;
            let column = if i % 2 == 0 { left } else { right };
            let row = Rect {
                y: column.y + (i / 2) as u16,
                height: 1,
                ..column
            };
            let gauge = LineGauge::default()
                .ratio((usage / 100.0).clamp(0.0, 1.0))
                .label(format!("{n:>2} {usage:>3.0}%"))
                .filled_symbol("█")
                .unfilled_symbol("░")
                .filled_style(color_bar(usage))
                .unfilled_style(DARK_GRAY);

            frame.render_widget(gauge, row);
        }

        let width = spark_area.width as usize;
        let skip = self.cpu_history.len().saturating_sub(width);
        let data: Vec<u64> = self
            .cpu_history
            .iter()
            .skip(skip)
            .map(|val| *val as u64)
            .collect();
        let spark_line = Sparkline::default().data(data).max(100).style(Color::Cyan);
        frame.render_widget(spark_line, spark_area);
    }

    fn render_memory(&self, frame: &mut Frame, area: Rect) {
        let block = style_block("Memory");
        let inner_block = block.inner(area);
        frame.render_widget(block, area);
        let mut constraints = vec![Constraint::Length(1), Constraint::Length(1)]; // RAM, SWAP
        constraints.extend(self.disks.iter().map(|_| Constraint::Length(1)));

        let mem_areas = Layout::vertical(constraints).spacing(1).split(inner_block);

        frame.render_widget(
            usage_gauge("RAM", self.system.used_memory(), self.system.total_memory()),
            mem_areas[0],
        );
        frame.render_widget(
            usage_gauge("SWAP", self.system.used_swap(), self.system.total_swap()),
            mem_areas[1],
        );
        for (disk, disk_area) in self.disks.iter().zip(&mem_areas[2..]) {
            let total = disk.total_space();
            let used = total - disk.available_space();
            let label = format!("Disk '{}'", disk.name().display());
            frame.render_widget(usage_gauge(&label, used, total), *disk_area);
        }
    }

    fn render_history(&self, frame: &mut Frame, area: Rect) {
        let cpu_points = to_points(&self.cpu_history);
        let ram_points = to_points(&self.ram_history);
        let data = vec![
            Dataset::default()
                .name("cpu")
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Color::Cyan)
                .data(&cpu_points),
            Dataset::default()
                .name("ram")
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(RAM_HISTORY)
                .data(&ram_points),
        ];
        let x_axis = Axis::default()
            .bounds([0.0, (HISTORY_LEN - 1) as f64])
            .labels(["60 sec", "30 sec", "now"])
            .style(DARK_GRAY);
        let y_axis = Axis::default()
            .bounds([0.0, 100.0])
            .labels(["0%", "50%", "100%"])
            .style(DARK_GRAY);
        let legend = Line::from(vec!["cpu".fg(Color::Cyan), "ram".fg(RAM_HISTORY)]).right_aligned();
        let block = style_block("History 60s").title_top(legend);
        let chart = Chart::new(data)
            .block(block)
            .x_axis(x_axis)
            .y_axis(y_axis)
            .legend_position(None);
        frame.render_widget(chart, area);
    }

    fn render_processes(&mut self, frame: &mut Frame, area: Rect) {
        let sort_name = match self.process_sort {
            SortBy::Cpu => "CPU",
            SortBy::Memory => "Memory",
        };
        let rows = self.processes.iter().map(|p| {
            Row::new([
                p.pid.to_string(),
                p.name.clone(),
                format!("{:.1}", p.cpu),
                format!("{} MB", p.memory_mb),
            ])
        });
        let header =
            Row::new(["PID", "Name", "CPU %", "MEM"]).style(Style::new().fg(Color::Cyan).bold());
        let widths = [
            Constraint::Length(8),
            Constraint::Fill(1),
            Constraint::Length(7),
            Constraint::Length(10),
        ];
        let table = Table::new(rows, widths)
            .header(header)
            .block(style_block(&format!("Processes sorted by {sort_name}")))
            .row_highlight_style(Style::new().bg(Color::Blue).fg(Color::White).bold())
            .highlight_symbol("▶ ");

        frame.render_stateful_widget(table, area, &mut self.process_state);
    }

    fn top_height(&self) -> u16 {
        let core_rows = self.system.cpus().len().div_ceil(2) as u16;
        let mem_rows = 2 + self.disks.len() as u16;

        let cpu_inner = (core_rows + 3).max(5);
        let mem_inner = mem_rows * 2 - 1;

        cpu_inner.max(mem_inner) + 2
    }
}

fn style_block<'a>(title: &str) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(DARK_GRAY)
        .title(Line::from(format!(" {title} ")).fg(TEXT_COLOR))
}

fn color_bar(value: f64) -> Color {
    if value < 50.0 {
        Color::Green
    } else if value < 80.0 {
        Color::Yellow
    } else {
        Color::Red
    }
}

fn calc_percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        used as f64 / total as f64 * 100.0
    }
}

fn gb(bytes: u64) -> f64 {
    bytes as f64 / 2f64.powi(30)
}

fn usage_gauge<'a>(name: &str, used: u64, total: u64) -> Gauge<'a> {
    let percent = calc_percent(used, total);
    Gauge::default()
        .ratio(percent / 100.0)
        .label(format!(
            "{name} {:.1} / {:.1} GB ({percent:.0}%)",
            gb(used),
            gb(total)
        ))
        .gauge_style(Style::new().fg(color_bar(percent)).bg(Color::Black))
        .use_unicode(true)
}

fn push_history(history: &mut VecDeque<f64>, value: f64) {
    if history.len() == HISTORY_LEN {
        history.pop_front();
    }
    history.push_back(value);
}

fn load_disks() -> Vec<Disk> {
    let mut seen = HashSet::new();
    let all = Vec::<Disk>::from(Disks::new_with_refreshed_list().expect("failed to load disks"));
    all.into_iter()
        .filter(|d| seen.insert((d.name().to_owned(), d.total_space(), d.available_space())))
        .collect()
}

fn to_points(history: &VecDeque<f64>) -> Vec<(f64, f64)> {
    let offset = HISTORY_LEN - history.len();
    history
        .iter()
        .enumerate()
        .map(|(i, val)| ((offset + i) as f64, *val))
        .collect()
}

fn main() -> std::io::Result<()> {
    ratatui::run(|terminal| Watcher::new().run(terminal))
}
