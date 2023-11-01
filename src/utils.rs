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

pub fn is_ips_file(entry: &DirEntry) -> bool {
    entry.file_type().is_dir()
        || entry
            .file_name()
            .to_str()
            .map(|s| s.to_ascii_uppercase().ends_with(".IPS"))
            .unwrap_or(false)
}

pub fn process_directory<Action, Filter, Dir>(
    action: Action,
    start_dir: Dir,
    filter: Filter,
    log: &mut dyn Write,
) -> ResultErr<()>
where
    Filter: FnMut(&DirEntry) -> bool,
    Action: Fn(DirEntry, &mut dyn Write) -> ResultErr<()>,
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
            let result = action(entry, log);
            match result {
                Ok(()) => (),
                Err(e) => {
                    eprintln!("Hit an error but continuing: {}", e);
                    writeln!(log, "Hit an error but continuing: {}", e)?;
                }
            }
        }
    }
    Ok(())
}
