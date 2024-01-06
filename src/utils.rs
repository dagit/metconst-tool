use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use walkdir::{DirEntry, WalkDir};

pub type ResultErr<T> = Result<T, Box<dyn std::error::Error>>;

pub fn open_log(fname: &str) -> ResultErr<BufWriter<File>> {
    let log = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(fname)?;
    Ok(BufWriter::new(log))
}

pub fn is_zip_file(entry: &DirEntry) -> bool {
    entry.file_type().is_dir()
        || entry
            .file_name()
            .to_str()
            .map(|s| s.to_ascii_uppercase().ends_with(".ZIP"))
            .unwrap_or(false)
}

pub fn is_rar_file(entry: &DirEntry) -> bool {
    entry.file_type().is_dir()
        || entry
            .file_name()
            .to_str()
            .map(|s| s.to_ascii_uppercase().ends_with(".RAR"))
            .unwrap_or(false)
}

pub fn is_7z_file(entry: &DirEntry) -> bool {
    entry.file_type().is_dir()
        || entry
            .file_name()
            .to_str()
            .map(|s| s.to_ascii_uppercase().ends_with(".7Z"))
            .unwrap_or(false)
}

pub fn is_archive_file(entry: &DirEntry) -> bool {
    is_zip_file(entry) || is_rar_file(entry) || is_7z_file(entry)
}

pub fn is_ips_file(entry: &DirEntry) -> bool {
    entry.file_type().is_dir()
        || entry
            .file_name()
            .to_str()
            .map(|s| s.to_ascii_uppercase().ends_with(".IPS"))
            .unwrap_or(false)
}

pub fn process_directory<Action, Filter, Dir>(
    mut action: Action,
    start_dir: Dir,
    filter: Filter,
    log: &mut dyn Write,
) -> ResultErr<()>
where
    Filter: FnMut(&DirEntry) -> bool,
    Action: FnMut(&DirEntry, &mut dyn Write) -> ResultErr<()>,
    Dir: AsRef<Path>,
{
    for entry in WalkDir::new(start_dir).into_iter().filter_entry(filter) {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("Skipping directory due to error: {}", e);
                writeln!(log, "Skipping directory due to error: {}", e)?;
                continue;
            }
        };
        //println!("{:?}", entry.path());
        if entry.file_type().is_file() {
            let result = action(&entry, log);
            match result {
                Ok(()) => (),
                Err(e) => {
                    eprintln!(
                        "Hit an error on {}, but continuing: {}",
                        entry.path().to_string_lossy(),
                        e
                    );
                    writeln!(
                        log,
                        "Hit an error on {}, but continuing: {}",
                        entry.path().to_string_lossy(),
                        e
                    )?;
                }
            }
        }
    }
    Ok(())
}
