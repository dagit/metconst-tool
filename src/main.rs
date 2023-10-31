use clap::Parser;
use ips::Patch;
use regex::Regex;
use scraper::{Html, Selector};
use std::fs::{self, create_dir_all, File, OpenOptions};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::PathBuf;
use walkdir::{DirEntry, WalkDir};

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    #[arg(short, long)]
    mode: RunMode,
    #[arg(short, long)]
    base_rom: Option<String>,
}

#[derive(clap::ValueEnum, Debug, Copy, Clone, PartialEq, Eq)]
enum RunMode {
    Download,
    Patch,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    println!("{:#?}", args);
    match args.mode {
        RunMode::Download => {
            download().await?;
        }
        RunMode::Patch => {
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .open("patch.txt")?;
            let mut log_writer = BufWriter::new(&log);
            if let Some(base_rom) = args.base_rom {
                patch(&base_rom, &mut log_writer)?;
            } else {
                println!(
                    "Must specify the base rom file to use for patching. Check --help output."
                );
            }
        }
    }

    Ok(())
}

fn is_ips_file(entry: &DirEntry) -> bool {
    entry.file_type().is_dir()
        || entry
            .file_name()
            .to_str()
            .map(|s| s.to_ascii_uppercase().ends_with(".IPS"))
            .unwrap_or(false)
}

fn patch(base_rom: &str, log: &mut dyn Write) -> Result<(), Box<dyn std::error::Error>> {
    for entry in WalkDir::new(".").into_iter().filter_entry(is_ips_file) {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("Skipping directory due to error: {}", e);
                writeln!(log, "Skipping directory due to error: {}", e)?;
                continue;
            }
        };
        if entry.file_type().is_file() {
            let result = patch_in_dir(base_rom, entry, log);
            match result {
                Ok(()) => (),
                Err(e) => {
                    eprintln!("Hit an error but continuing: {}", e);
                }
            }
        }
    }
    Ok(())
}

fn patch_in_dir(
    base_rom: &str,
    entry: DirEntry,
    log: &mut dyn Write,
) -> Result<(), Box<dyn std::error::Error>> {
    let dir_path = entry.path().parent().ok_or("bad path")?;
    let mut rom_file = PathBuf::new();
    rom_file.push("patched");
    rom_file.push(dir_path);
    fs::create_dir_all(&rom_file)?;
    rom_file.push(entry.file_name());
    let extension = base_rom.rsplit_once('.');
    rom_file.set_extension(extension.map(|(_, e)| e).unwrap());

    writeln!(
        log,
        "Applying {} to create {}, in {}",
        entry.path().to_str().unwrap_or("error"),
        rom_file.to_str().unwrap_or("error"),
        dir_path.to_str().unwrap_or("error"),
    )?;

    // Create a clean copy of the rom
    writeln!(log, "Copying {:#?} to {:#?}", base_rom, &rom_file)?;
    fs::copy(base_rom, &rom_file)?;
    // Ensure that we can write to it
    writeln!(log, "Setting permissions on {:#?}", &rom_file)?;
    let mut perms = fs::metadata(&rom_file)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&rom_file, perms)?;

    // Open the rom file and begin overwriting it
    writeln!(log, "Opening {:#?} to apply patch", &rom_file)?;
    let mut rom = OpenOptions::new().write(true).append(true).open(rom_file)?;
    writeln!(log, "Reading patch file {:#?}", &entry.path())?;
    let patch_contents = fs::read(entry.path())?;
    let patch = Patch::parse(&patch_contents)?;

    writeln!(log, "Applying hunks")?;
    for hunk in patch.hunks() {
        rom.seek(SeekFrom::Start(hunk.offset() as u64))?;
        rom.write_all(hunk.payload())?;
    }

    if let Some(truncation) = patch.truncation() {
        writeln!(log, "Truncating")?;
        rom.set_len(truncation as u64)?;
    }

    Ok(())
}

async fn download() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::ClientBuilder::new().user_agent("Foo").build()?;
    let metconst = "https://metroidconstruction.com/";

    // TODO: this will need to pull down mulitple pages once there are > 1000 hacks
    let allhacks = format!("{}hacks.php?sort=5&dir=asc&filters%5B%5D=SM&filters%5B%5D=Unknown&filters%5B%5D=Boss+Rush&filters%5B%5D=Exploration&filters%5B%5D=Challenge&filters%5B%5D=Spoof&filters%5B%5D=Speedrun%2FRace&filters%5B%5D=Incomplete&filters%5B%5D=Quick+Play&filters%5B%5D=Improvement&filters%5B%5D=Vanilla%2B&search=&num_per_page=1000", metconst);

    let body = client.get(allhacks).send().await?.text().await?;
    let document = Html::parse_document(&body);
    let row_selector = Selector::parse("td")?;
    let ahref = Selector::parse("a")?;

    // example: hack.php?id=756
    let re = Regex::new(r"^hack\.php\?id=([0-9]+)$")?;

    let mut hack_id = Vec::new();
    for element in document.select(&row_selector) {
        for e in element.select(&ahref) {
            if let Some(href) = e.value().attr("href") {
                for (_, [id]) in re.captures_iter(href).map(|c| c.extract()) {
                    hack_id.push(id);
                }
            }
        }
    }

    for (idx, id) in hack_id.iter().enumerate() {
        let hack_url = format!("{}hack.php?id={}", metconst, id);
        let hack_page = client.get(hack_url).send().await?.text().await?;
        let document = Html::parse_document(&hack_page);
        let download_link = format!(r"(^download\.php\?id={})", id);
        let re = Regex::new(&download_link)?;

        for element in document.select(&ahref) {
            if let Some(href) = element.value().attr("href") {
                //println!("href={}", href);
                if re.is_match(href) {
                    let redirect_url = format!("{}{}", metconst, href);
                    let redirect_contents = client.get(redirect_url).send().await?.text().await?;
                    let meta = Selector::parse("meta")?;
                    let document = Html::parse_document(&redirect_contents);
                    for element in document.select(&meta) {
                        if let Some(url) = element.value().attr("content") {
                            if let Some((_, url)) = url.rsplit_once('=') {
                                if let Some((_, file_name)) = url.rsplit_once('/') {
                                    println!("url: {}", url);
                                    println!("file_name: {}", file_name);
                                    let file_contents =
                                        client.get(url).send().await?.bytes().await?;
                                    let dir_name = format!("downloads/{:04}-{}", idx, id);
                                    println!("dir_name: {}", dir_name);
                                    create_dir_all(&dir_name)?;
                                    let file_name = format!("{}/{}", dir_name, file_name);
                                    let mut file = File::create(file_name)?;
                                    file.write_all(&file_contents)?;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
