/// Concatenate two paths, even if the right argument is an absolute path.
fn join_absolute<A: AsRef<Path>, B: AsRef<Path>>(left: A, right: B) -> PathBuf {
    let (left, right) = (left.as_ref(), right.as_ref());
    left.join(
        if let Ok(stripped) = right.strip_prefix("/") {
            stripped
        } else {
            right
        }
    )
}

fn symlink_exists<P: AsRef<Path>>(path: P) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn get_user() -> String {
    env::var("USER").unwrap_or(String::from("unknown"))
}

/// Prompt for user input, returning True if the first character is 'y' or 'Y'
fn prompt_yes<T: AsRef<str>>(prompt: T) -> bool {
    print!("{} (y/N) ", prompt.as_ref());
    if io::stdout().flush().is_err() {
        // If stdout wasn't flushed properly, fallback to println
        println!("{} (y/N)", prompt.as_ref());
    }
    let stdin = BufReader::new(io::stdin());
    if let Some(c) = stdin.chars().next() {
        if let Ok(c) = c {
            return c == 'y' || c == 'Y';
        }
    }
    false
}

/// Add a numbered extension to duplicate filenames to avoid overwriting files.
fn rename_grave<G: AsRef<Path>>(grave: G) -> PathBuf {
    let grave = grave.as_ref();
    let name = grave.to_str().expect("Filename must be valid unicode.");
    (1_u64..)
        .map(|i| PathBuf::from(format!("{}~{}", name, i)))
        .skip_while(|p| symlink_exists(p))
        .next()
        .expect("Failed to rename duplicate file or directory")
}

fn humanize_bytes(bytes: u64) -> String {
    let values = ["bytes", "KB", "MB", "GB", "TB"];
    let pair = values.iter()
        .enumerate()
        .take_while(|x| bytes as usize / (1000 as usize).pow(x.0 as u32) > 10)
        .last();
    if let Some((i, unit)) = pair {
        format!("{} {}", bytes as usize / (1000 as usize).pow(i as u32), unit)
    } else {
        format!("{} {}", bytes, values[0])
    }
}
