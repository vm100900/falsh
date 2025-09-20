use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use glob::glob;
use rustyline::completion::{Completer, FilenameCompleter, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::error::ReadlineError;
use rustyline::{Editor, Helper, Context};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
};
use dirs;

/// ------------------ HELPER FOR AUTOCOMPLETE ------------------
struct FalshHelper {
    file_comp: FilenameCompleter,
    builtins: Vec<String>,
}
impl Helper for FalshHelper {}
impl Completer for FalshHelper {
    type Candidate = Pair;
    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let (start, word) = match line[..pos].rfind(|c: char| c.is_whitespace()) {
            Some(idx) => (idx + 1, &line[idx + 1..pos]),
            None => (0, &line[..pos]),
        };
        let mut out = Vec::new();
        if start == 0 {
            for b in &self.builtins {
                if b.starts_with(word) {
                    out.push(Pair {
                        display: b.clone(),
                        replacement: b.clone(),
                    });
                }
            }
        }
        let (_, mut files) = self.file_comp.complete(line, pos, ctx)?;
        out.append(&mut files);
        Ok((start, out))
    }
}
impl Hinter for FalshHelper {
    type Hint = String;
    fn hint(&self, _: &str, _: usize, _: &Context<'_>) -> Option<String> { None }
}
impl Highlighter for FalshHelper {}
impl Validator for FalshHelper {}
/// -------------------------------------------------------------

fn split_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = input.chars().peekable();

    while let Some(&ch) = chars.peek() {
        match ch {
            '\'' if !in_double => { in_single = !in_single; chars.next(); }
            '"' if !in_single => { in_double = !in_double; chars.next(); }
            ' ' if !in_single && !in_double => {
                if !current.is_empty() { args.push(current.clone()); current.clear(); }
                chars.next();
            }
            _ => { current.push(ch); chars.next(); }
        }
    }
    if !current.is_empty() { args.push(current); }
    args
}

fn expand_globs(args: Vec<String>) -> Vec<String> {
    let mut expanded = Vec::new();
    for arg in args {
        if arg.contains('*') || arg.contains('?') {
            for entry in glob(&arg).unwrap().filter_map(Result::ok) {
                expanded.push(entry.to_string_lossy().to_string());
            }
        } else {
            expanded.push(arg);
        }
    }
    expanded
}

fn change_dir(path: &str) {
    if let Err(e) = env::set_current_dir(path) {
        println!("cd failed: {}", e);
    }
}

fn print_working_dir() {
    match env::current_dir() {
        Ok(path) => println!("{}", path.display()),
        Err(e) => println!("pwd failed: {}", e),
    }
}

fn get_persistent_path_file() -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push(".falsh_path");
    path
}

fn get_falshrc_file() -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push(".falshrc");
    path
}

fn load_persistent_paths() -> Vec<String> {
    let file = get_persistent_path_file();
    if !file.exists() { return vec![]; }
    BufReader::new(File::open(file).unwrap())
        .lines()
        .filter_map(Result::ok)
        .collect()
}

fn save_persistent_paths(paths: &[String]) {
    let file = get_persistent_path_file();
    let mut f = File::create(file).unwrap();
    for p in paths { writeln!(f, "{}", p).unwrap(); }
}

fn add_to_path(user_input: &str, temporary: bool) {
    let actual_path = PathBuf::from(user_input);
    let path_to_add = match fs::metadata(&actual_path) {
        Ok(meta) => {
            if meta.is_file() {
                actual_path.parent().map(|p| p.to_path_buf()).unwrap_or(actual_path.clone())
            } else { actual_path.clone() }
        }
        Err(_) => {
            println!("Warning: path {} does not exist.", user_input);
            actual_path.clone()
        }
    };

    let add_str = path_to_add.to_string_lossy().to_string();

    if !temporary {
        let mut paths = load_persistent_paths();
        if !paths.iter().any(|p| p == user_input) {
            paths.push(user_input.to_string());
            save_persistent_paths(&paths);
        }
    }

    let mut path_env = env::var("PATH").unwrap_or_default();
    if !path_env.split(':').any(|p| p == add_str) {
        if !path_env.is_empty() { path_env.push(':'); }
        path_env.push_str(&add_str);
        unsafe{
        env::set_var("PATH", &path_env);
        }
    }
}

fn prompt_line(prompt: &str) -> Option<String> {
    disable_raw_mode().ok();
    let mut rl = Editor::<(), rustyline::history::DefaultHistory>::new().ok()?;
    let out = rl.readline(&format!("\r{}", prompt)).ok();
    enable_raw_mode().ok();
    out.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn list_path() {
    let mut paths = load_persistent_paths();
    enable_raw_mode().unwrap();
    let mut selected: usize = 0;

    loop {
        execute!(io::stdout(), Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
        println!("Use ↑/↓ to navigate, Enter to delete/open '+', Esc to exit:\n");

        for (i, path) in paths.iter().enumerate() {
            if i == selected { println!("> {}", path); }
            else { println!("  {}", path); }
        }
        let plus_idx = paths.len();
        if selected == plus_idx { println!("> [+ Add new path]"); }
        else { println!("  [+ Add new path]"); }

        if let Event::Key(KeyEvent { code, .. }) = event::read().unwrap() {
            match code {
                KeyCode::Up => { if selected > 0 { selected -= 1; } else { selected = plus_idx; } }
                KeyCode::Down => { if selected < plus_idx { selected += 1; } else { selected = 0; } }
                KeyCode::Enter => {
                    if selected == plus_idx {
                        if let Some(newp) = prompt_line("Enter path to add: ") {
                            add_to_path(&newp, false);
                            paths = load_persistent_paths();
                        }
                    } else if !paths.is_empty() {
                        paths.remove(selected);
                        save_persistent_paths(&paths);
                        if selected >= paths.len() && selected > 0 { selected -= 1; }
                    }
                }
                KeyCode::Esc => break,
                _ => {}
            }
        }
    }
    disable_raw_mode().unwrap();
}

fn load_persistent_into_env() {
    for user_entry in load_persistent_paths() {
        add_to_path(&user_entry, true);
    }
}

fn execute_line(input: &str) {
    if input.is_empty() { return; }

    let pipeline: Vec<&str> = input.split('|').map(|s| s.trim()).collect();
    let mut previous_output: Option<Stdio> = None;

    for (i, segment) in pipeline.iter().enumerate() {
        let mut args = split_args(segment);
        if args.is_empty() { continue; }

        if args[0] == "cd" {
            if args.len() > 1 { change_dir(&args[1]); } else { println!("cd: missing argument"); }
            continue;
        } else if args[0] == "pwd" { print_working_dir(); continue; }
        else if args[0] == "addToPath" {
            let temporary = args.iter().any(|a| a == "--temp");
            if args.len() > 1 { add_to_path(&args[1], temporary); } else { println!("addToPath: missing argument"); }
            continue;
        } else if args[0] == "pathTool" { list_path(); continue; }

        let mut stdin_source = previous_output.unwrap_or(Stdio::inherit());
        let mut stdout_target = Stdio::inherit();

        if let Some(pos) = args.iter().position(|x| x == ">") {
            if pos + 1 < args.len() {
                let filename = &args[pos + 1];
                stdout_target = Stdio::from(File::create(filename).unwrap());
                args.truncate(pos);
            }
        }
        if let Some(pos) = args.iter().position(|x| x == "<") {
            if pos + 1 < args.len() {
                let filename = &args[pos + 1];
                stdin_source = Stdio::from(File::open(filename).unwrap());
                args.truncate(pos);
            }
        }

        let args_expanded = expand_globs(args[1..].to_vec());

        let child = Command::new(&args[0])
            .args(&args_expanded)
            .stdin(stdin_source)
            .stdout(if i < pipeline.len() - 1 { Stdio::piped() } else { stdout_target })
            .spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => { println!("Command failed: {}", e); break; }
        };

        previous_output = child.stdout.take().map(Stdio::from);
        child.wait().unwrap();
    }
}

fn load_falshrc() {
    let file = get_falshrc_file();
    if !file.exists() { return; }
    if let Ok(lines) = BufReader::new(File::open(file).unwrap()).lines().collect::<Result<Vec<_>, _>>() {
        for line in lines {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') { continue; } // ignore empty and comments
            execute_line(trimmed);
        }
    }
}

fn main() -> rustyline::Result<()> {
    load_persistent_into_env();
    println!("Running in \x1B[1;3;38;5;214mfalsh\x1B[0m");
    load_falshrc(); // load ~/.falshrc at startup

    let builtins = vec![
        "cd".to_string(),
        "pwd".to_string(),
        "addToPath".to_string(),
        "listPaths".to_string(),
        "exit".to_string(),
    ];

    let helper = FalshHelper {
        file_comp: FilenameCompleter::new(),
        builtins,
    };

    let mut rl = Editor::<FalshHelper, rustyline::history::DefaultHistory>::new()?;
    rl.set_helper(Some(helper));

    loop {
        // Dynamic prompt showing **full absolute path**
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("?"));
let prompt = format!("{}> ", cwd.display());
        let readline = rl.readline(&prompt);
        let input = match readline {
            Ok(line) => { let _ = rl.add_history_entry(line.as_str()); line.trim().to_string() },
            Err(ReadlineError::Interrupted) => { println!("^C"); continue; },
            Err(ReadlineError::Eof) => break,
            Err(err) => { println!("Error: {:?}", err); break; }
        };

        if input.is_empty() { continue; }
        if input == "exit" { break; }

        execute_line(&input);
    }

    Ok(())
}
