fn symlink_exists<P: AsRef<Path>>(path: P) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn get_user() -> String {
    env::var("USER").unwrap_or(String::from("unknown"))
}

/// Prompt for user input, returning True if the first character is 'y' or 'Y'
fn prompt_yes(prompt: &str) -> bool {
    print!("{} (y/n) ", prompt);
    io::stdout().flush().unwrap();
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
    if grave.extension().is_none() {
        (1_u64..)
            .map(|i| grave.with_extension(i.to_string()))
            .skip_while(|p| symlink_exists(p))
            .next()
            .expect("Failed to rename duplicate file or directory")
    } else {
        (1_u64..)
            .map(|i| {
                grave.with_extension(format!("{}.{}",
                                             grave.extension()
                                             .unwrap()
                                             .to_str()
                                             .unwrap(),
                                             i))
            })
            .skip_while(|p| symlink_exists(p))
            .next()
            .expect("Failed to rename duplicate file or directory")
    }
}
