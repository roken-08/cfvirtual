use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{
    io::{self, IsTerminal, Stdout},
    panic::PanicHookInfo,
    sync::{atomic::AtomicBool, atomic::Ordering, Arc},
};

pub(super) type Tty = Terminal<CrosstermBackend<Stdout>>;
type Hook = Arc<dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static>;
static ACTIVE: AtomicBool = AtomicBool::new(false);
static PANICKED: AtomicBool = AtomicBool::new(false);

pub(super) fn check() -> Result<()> {
    anyhow::ensure!(
        !PANICKED.load(Ordering::SeqCst),
        "A background operation stopped unexpectedly; terminal restored"
    );
    Ok(())
}

pub(super) struct Session {
    pub terminal: Tty,
    previous_hook: Hook,
}

fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
}

impl Session {
    pub fn enter() -> Result<Self> {
        anyhow::ensure!(
            io::stdin().is_terminal() && io::stdout().is_terminal(),
            "An interactive terminal is required (use the headless demo or read commands when redirected)"
        );
        anyhow::ensure!(
            !ACTIVE.swap(true, Ordering::SeqCst),
            "A terminal app is already running"
        );
        // Every partially successful initialization also restores the terminal.
        let setup = (|| -> Result<Tty> {
            enable_raw_mode().context("enable terminal keyboard input")?;
            execute!(io::stdout(), EnterAlternateScreen, Hide)?;
            let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
            terminal.clear()?;
            Ok(terminal)
        })();
        let terminal = match setup {
            Ok(terminal) => terminal,
            Err(error) => {
                restore();
                ACTIVE.store(false, Ordering::SeqCst);
                return Err(error);
            }
        };
        let previous_hook: Hook = Arc::from(std::panic::take_hook());
        let previous = Arc::clone(&previous_hook);
        PANICKED.store(false, Ordering::SeqCst);
        std::panic::set_hook(Box::new(move |info| {
            PANICKED.store(true, Ordering::SeqCst);
            restore();
            if std::env::var_os("RUST_BACKTRACE").is_some_and(|value| value == "1") {
                previous(info);
            } else {
                eprintln!("CFVirtual encountered an unexpected error; terminal restored.");
            }
        }));
        Ok(Self {
            terminal,
            previous_hook,
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        restore();
        ACTIVE.store(false, Ordering::SeqCst);
        if !std::thread::panicking() {
            let previous = Arc::clone(&self.previous_hook);
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}

/// Cancels network workers even when input/rendering fails or unwinds.
pub(super) struct Worker(pub tokio::task::JoinHandle<()>);

impl Drop for Worker {
    fn drop(&mut self) {
        self.0.abort();
    }
}
