use clap::Parser;
use indicatif::ProgressBar;
use ips::Patch;
use regex::Regex;
use reqwest_middleware::ClientBuilder;
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use sanitise_file_name::sanitise;
use scraper::{Html, Selector};
use std::fs::{self, create_dir_all, File, OpenOptions};
use std::io::{BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::PathBuf;
use walkdir::DirEntry;

mod utils;
use utils::*;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    #[command(subcommand)]
    mode: RunMode,
}

#[derive(clap::Subcommand, Debug, Clone, PartialEq, Eq)]
enum RunMode {
    Download,
    Patch(PatchArgs),
    Unzip,
    FileTypes,
    Metadata,
}

#[derive(clap::Args, Debug, Clone, PartialEq, Eq)]
struct PatchArgs {
    #[arg()]
    base_rom: String,
}

#[tokio::main]
async fn main() -> ResultErr<()> {
    let args = Args::parse();

    match args.mode {
        RunMode::Download => {
            let mut log_writer = open_log("download.txt")?;
            download(&mut log_writer).await?;
        }
        RunMode::Unzip => {
            let mut log_writer = open_log("unzip.txt")?;
            process_directory(
                unarchive_in_dir,
                "downloads",
                is_archive_file,
                &mut log_writer,
            )?;
        }
        RunMode::Patch(pa) => {
            let mut log_writer = open_log("patch.txt")?;
            process_directory(
                |f, l| patch_in_dir(&pa.base_rom, f, l),
                "downloads",
                is_ips_file,
                &mut log_writer,
            )?;
        }
        RunMode::FileTypes => {
            use std::collections::HashSet;
            let mut log_writer = open_log("filetypes.txt")?;
            let mut extensions: HashSet<String> = HashSet::new();
            process_directory(
                |f, _| {
                    let path = f.path();
                    if let Some(ext) = path.extension() {
                        extensions.insert(ext.to_string_lossy().to_string().to_lowercase());
                    }
                    Ok(())
                },
                "downloads",
                |_| true,
                &mut log_writer,
            )?;
            println!("extensions: {:?}", extensions);
        }
        RunMode::Metadata => {
            let mut log_writer = open_log("metadata.txt")?;
            metadata(&mut log_writer).await?;
        }
    }

    Ok(())
}

fn unarchive_in_dir(entry: &DirEntry, log: &mut dyn Write) -> ResultErr<()> {
    if is_zip_file(entry) {
        unzip_in_dir(entry, log)?
    } else if is_rar_file(entry) {
        unrar_in_dir(entry, log)?
    } else if is_7z_file(entry) {
        un7z_in_dir(entry, log)?
    }
    Ok(())
}

fn un7z_in_dir(entry: &DirEntry, log: &mut dyn Write) -> ResultErr<()> {
    writeln!(log, "7z file: {:?}", entry.path()).expect("cannot write to log");
    if let Some(parent) = entry.path().parent() {
        if let Some(archive_name) = entry.path().file_stem() {
            let mut unpack_dir = PathBuf::new();
            unpack_dir.push(parent);
            unpack_dir.push(archive_name);
            create_dir_all(&unpack_dir)?;
            writeln!(log, "Creating: {:?}", unpack_dir).expect("failed to write to log");
            sevenz_rust::decompress_file(entry.path(), unpack_dir)?;
        }
    }
    Ok(())
}

fn unrar_in_dir(entry: &DirEntry, log: &mut dyn Write) -> ResultErr<()> {
    writeln!(log, "Rar file: {:?}", entry.path()).expect("cannot write to log");
    let mut archive = unrar::Archive::new(entry.path()).open_for_processing()?;
    if let Some(parent) = entry.path().parent() {
        if let Some(archive_name) = entry.path().file_stem() {
            let mut unpack_dir = PathBuf::new();
            unpack_dir.push(parent);
            unpack_dir.push(archive_name);
            while let Some(header) = archive.read_header()? {
                archive = if header.entry().is_file() {
                    let mut full_file_name = PathBuf::new();
                    full_file_name.push(unpack_dir.clone());
                    full_file_name.push(&header.entry().filename);

                    create_dir_all(full_file_name.parent().unwrap())?;

                    writeln!(log, "Creating: {:?}", full_file_name)
                        .expect("failed to write to log");
                    header.extract_with_base(full_file_name.parent().unwrap())?
                } else {
                    header.skip()?
                };
            }
        }
    }
    Ok(())
}

fn unzip_in_dir(entry: &DirEntry, log: &mut dyn Write) -> ResultErr<()> {
    writeln!(log, "Zip file: {:?}", entry.path()).expect("cannot write to log");
    let zip_file = File::open(entry.path())?;
    let zip_reader = BufReader::new(&zip_file);

    let mut zip = zip::ZipArchive::new(zip_reader)?;

    if let Some(parent) = entry.path().parent() {
        if let Some(zip_name) = entry.path().file_stem() {
            let mut unpack_dir = PathBuf::new();
            unpack_dir.push(parent);
            unpack_dir.push(zip_name);

            writeln!(log, "creating unpack directory: {:?}", unpack_dir)
                .expect("failed to write log");
            create_dir_all(&unpack_dir)?;

            for i in 0..zip.len() {
                let mut file = zip.by_index(i)?;
                if file.name().ends_with('/') {
                    continue;
                }
                let mut full_file_name = PathBuf::new();
                full_file_name.push(unpack_dir.clone());
                full_file_name.push(file.name());

                create_dir_all(full_file_name.parent().unwrap())?;

                writeln!(log, "Creating: {:?}", full_file_name).expect("failed to write to log");
                let output = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(full_file_name)?;
                let mut output_writer = BufWriter::new(&output);
                std::io::copy(&mut file, &mut output_writer)?;
            }
        }
    }
    Ok(())
}

fn patch_in_dir(base_rom: &str, entry: &DirEntry, log: &mut dyn Write) -> ResultErr<()> {
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
    let mut rom = OpenOptions::new().read(true).write(true).open(rom_file)?;
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

async fn download(log: &mut dyn Write) -> ResultErr<()> {
    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(10);
    let client = ClientBuilder::new(reqwest::ClientBuilder::new().user_agent("Foo").build()?)
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build();
    let metconst = "https://metroidconstruction.com/";

    // TODO: this will need to pull down mulitple pages once there are > 1000 hacks
    let allhacks = format!("{}hacks.php?sort=5&dir=asc&filters%5B%5D=SM&filters%5B%5D=Unknown&filters%5B%5D=Boss+Rush&filters%5B%5D=Exploration&filters%5B%5D=Challenge&filters%5B%5D=Spoof&filters%5B%5D=Speedrun%2FRace&filters%5B%5D=Incomplete&filters%5B%5D=Quick+Play&filters%5B%5D=Improvement&filters%5B%5D=Vanilla%2B&search=&num_per_page=1000", metconst);

    println!("Fetching list of hacks...");
    let body = client.get(allhacks).send().await?.text().await?;
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
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
    println!(
        "There are a total of {} hacks available. This process may take several hours.",
        hack_id.len()
    );

    let pb = ProgressBar::new(hack_id.len() as u64);

    for (idx, id) in hack_id.iter().enumerate() {
        let hack_url = format!("{}hack.php?id={}", metconst, id);
        let hack_page = client.get(hack_url).send().await?.text().await?;
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        let document = Html::parse_document(&hack_page);
        let download_link = format!(r"(^download\.php\?id={})", id);
        let re = Regex::new(&download_link)?;
        let meta = Selector::parse("meta")?;
        #[allow(non_snake_case)]
        let underboxA = Selector::parse("td.underboxA")?;

        // Extract hack title
        // In an ideal world, we would always just use the meta property
        // but for some reason, not all hack pages have that attribute set.
        // So when we can't find the meta tag with "og:title" we fallback to
        // looking for the hack title on the page
        let mut title = None;
        for element in document.select(&meta) {
            if element.attr("property") == Some("og:title") {
                title = element.attr("content");
            }
        }
        if title.is_none() {
            // We just want the first underboxA on the page
            if let Some(element) = document.select(&underboxA).next() {
                title = element.text().next().map(|t| t.trim());
            }
        }
        // No longer mutable
        let title = title;

        for element in document.select(&ahref) {
            if let Some(href) = element.value().attr("href") {
                //println!("href={}", href);
                if re.is_match(href) {
                    let redirect_url = format!("{}{}", metconst, href);
                    let redirect_contents = client.get(redirect_url).send().await?.text().await?;
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    let document = Html::parse_document(&redirect_contents);
                    for element in document.select(&meta) {
                        if let Some(url) = element.value().attr("content") {
                            if let Some((_, url)) = url.rsplit_once('=') {
                                if let Some((_, file_name)) = url.rsplit_once('/') {
                                    let dir_name;
                                    if let Some(title) = title {
                                        dir_name = format!(
                                            "downloads/{}",
                                            sanitise(&format!("{:04}-{}-{}", idx, id, title))
                                        );
                                    } else {
                                        dir_name = format!("downloads/{:04}-{}", idx, id);
                                    }
                                    let full_file_name = format!("{}/{}", dir_name, file_name);
                                    if std::path::Path::new(&full_file_name).exists() {
                                        //println!("skipping {}, already downloaded", url);
                                        writeln!(log, "skipping {}, already downloaded", url)
                                            .expect("failed to log");
                                    } else {
                                        //println!("url: {}", url);
                                        writeln!(log, "url: {}", url).expect("failed to log");
                                        //println!("file_name: {}", file_name);
                                        writeln!(log, "file_name: {}", file_name)
                                            .expect("failed to log");
                                        let file_contents =
                                            client.get(url).send().await?.bytes().await?;
                                        //println!("dir_name: {}", dir_name);
                                        writeln!(log, "dir_name: {}", dir_name)
                                            .expect("failed to log");
                                        create_dir_all(&dir_name)?;
                                        let mut file = File::create(full_file_name)?;
                                        file.write_all(&file_contents)?;
                                        tokio::time::sleep(tokio::time::Duration::from_secs(5))
                                            .await;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        pb.inc(1);
    }
    pb.finish_with_message("done");

    Ok(())
}

async fn metadata(_: &mut dyn Write) -> ResultErr<()> {
    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(10);
    let client = ClientBuilder::new(reqwest::ClientBuilder::new().user_agent("Foo").build()?)
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build();
    let metconst = "https://metroidconstruction.com/";

    // TODO: this will need to pull down mulitple pages once there are > 1000 hacks
    let allhacks = format!("{}hacks.php?sort=5&dir=asc&filters%5B%5D=SM&filters%5B%5D=Unknown&filters%5B%5D=Boss+Rush&filters%5B%5D=Exploration&filters%5B%5D=Challenge&filters%5B%5D=Spoof&filters%5B%5D=Speedrun%2FRace&filters%5B%5D=Incomplete&filters%5B%5D=Quick+Play&filters%5B%5D=Improvement&filters%5B%5D=Vanilla%2B&search=&num_per_page=1000", metconst);

    println!("Fetching list of hacks...");
    let body = client.get(allhacks).send().await?.text().await?;
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
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
    println!("There are a total of {} hacks available.", hack_id.len());

    let pb = ProgressBar::new(hack_id.len() as u64);

    // Release date:
    let release_date_re = Regex::new(r"<b>Release date:</b>(.*)")?;
    // Author:
    let author_re = Regex::new("<b>Author:</b> <a href=\".*\">(.*)</a>")?;
    // Genre:
    let genre_re = Regex::new("<b>Genre:</b> (.*) <")?;
    // Difficulty:
    let difficulty_re = Regex::new("<b>Difficulty:</b> (.*) <")?;
    let mut csv_writer = open_log("metadata.csv")?;
    let rating_re = Regex::new("Average Rating: ([0-9]+.[0-9]+) chozo orbs")?;
    writeln!(
        csv_writer,
        "title,date,author,genre,difficulty,avg runtime,avg collection,avg rating,by pedro"
    )?;
    let mut pedro_aliases = vec![
        "crimsonsunbird".to_owned(),
        "Juan Dennys".to_owned(),
        "pedro123".to_owned(),
        "jailsonmendes".to_owned(),
        "FaiskaBr".to_owned(),
    ];
    pedro_aliases
        .iter_mut()
        .for_each(|s| s.make_ascii_lowercase());
    for id in hack_id.iter() {
        let hack_url = format!("{}hack.php?id={}", metconst, id);
        let hack_page = client.get(hack_url).send().await?.text().await?;
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        let document = Html::parse_document(&hack_page);
        let meta = Selector::parse("meta")?;
        #[allow(non_snake_case)]
        let underboxA = Selector::parse("td.underboxA")?;
        #[allow(non_snake_case)]
        let underboxD = Selector::parse(".underboxD")?;

        // Extract hack title
        // In an ideal world, we would always just use the meta property
        // but for some reason, not all hack pages have that attribute set.
        // So when we can't find the meta tag with "og:title" we fallback to
        // looking for the hack title on the page
        let mut title = None;
        for element in document.select(&meta) {
            if element.attr("property") == Some("og:title") {
                title = element.attr("content");
            }
        }
        if title.is_none() {
            // We just want the first underboxA on the page
            if let Some(element) = document.select(&underboxA).next() {
                title = element.text().next().map(|t| t.trim());
            }
        }
        // No longer mutable
        let title = title;

        let mut date: String = String::new();
        let mut author: String = String::new();
        let mut genre = String::new();
        let mut difficulty = String::new();
        for element in document.select(&underboxD) {
            let text = element.inner_html();
            for (_, [d]) in release_date_re.captures_iter(&text).map(|c| c.extract()) {
                date = d.trim().to_owned();
            }
            for (_, [a]) in author_re.captures_iter(&text).map(|c| c.extract()) {
                author = a.trim().to_owned();
            }
            for (_, [g]) in genre_re.captures_iter(&text).map(|c| c.extract()) {
                genre = g.trim().to_owned();
            }
            for (_, [d]) in difficulty_re.captures_iter(&text).map(|c| c.extract()) {
                difficulty = d.trim().to_owned();
            }
        }
        let mut runtime = String::new();
        let avg_runtime = Selector::parse("#average_runtime")?;
        for element in document.select(&avg_runtime) {
            runtime = element.inner_html();
        }
        let mut collection = String::new();
        let avg_collection = Selector::parse("#average_completion")?;
        for element in document.select(&avg_collection) {
            collection = element.inner_html();
        }
        let mut rating = String::new();
        let avg_rating = Selector::parse("span[title]")?;
        for element in document.select(&avg_rating) {
            let text = element.inner_html();
            for (_, [d]) in rating_re.captures_iter(&text).map(|c| c.extract()) {
                rating = d.trim().to_owned();
            }
        }
        let by_pedro = if pedro_aliases.contains(&author.to_ascii_lowercase()) {
            "Y"
        } else {
            "N"
        };
        writeln!(
            csv_writer,
            "\"{}\",\"{}\",\"{}\",{},{},{},{},{},{}",
            title.unwrap_or(""),
            date,
            author,
            genre,
            difficulty,
            runtime,
            collection,
            rating,
            by_pedro,
        )?;
        pb.inc(1);
    }
    pb.finish_with_message("done");

    Ok(())
}
