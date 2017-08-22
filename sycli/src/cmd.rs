use std::{fs, cmp};
use std::path::Path;
use std::io::{self, Read};

use reqwest::{Client as HClient, header};
use serde_json;
use prettytable::Table;

use rpc::message::{CMessage, SMessage};
use rpc::criterion::{Criterion, Value, Operation};
use rpc::resource::{Resource, ResourceKind, SResourceUpdate, CResourceUpdate, Status};

use client::Client;
use error::{Result, ResultExt, ErrorKind};

pub fn add(mut c: Client, url: &str, files: Vec<&str>, dir: Option<&str>) -> Result<()> {
    for file in files {
        add_file(&mut c, url, file, dir)?;
    }
    Ok(())
}

fn add_file(c: &mut Client, url: &str, file: &str, dir: Option<&str>) -> Result<()> {
    let mut torrent = Vec::new();
    let mut f = fs::File::open(file).chain_err(|| ErrorKind::FileIO)?;
    f.read_to_end(&mut torrent).chain_err(|| ErrorKind::FileIO)?;

    let msg = CMessage::UploadTorrent {
        serial: c.next_serial(),
        size: torrent.len() as u64,
        path: dir.as_ref().map(|d| format!("{}", d)),
    };
    let token = if let SMessage::TransferOffer { token, .. } = c.rr(msg)? {
        token
    } else {
        bail!("Failed to receieve transfer offer from synapse!");
    };
    let client = HClient::new().chain_err(|| ErrorKind::HTTP)?;
    client
        .post(url)
        .chain_err(|| ErrorKind::HTTP)?
        .header(header::Authorization(header::Bearer { token }))
        .body(torrent)
        .send()
        .chain_err(|| ErrorKind::HTTP)?;

    if let SMessage::OResourcesExtant { .. } = c.recv()? {
    } else {
        bail!("Failed to receieve upload acknowledgement from synapse!");
    };

    Ok(())
}

pub fn del(mut c: Client, torrents: Vec<&str>) -> Result<()> {
    for torrent in torrents {
        del_torrent(&mut c, torrent)?;
    }
    Ok(())
}

fn del_torrent(c: &mut Client, torrent: &str) -> Result<()> {
    let resources = search_torrent_name(c, torrent)?;
    if resources.len() == 1 {
        let msg = CMessage::RemoveResource {
            serial: c.next_serial(),
            id: resources[0].id().to_owned(),
        };
        c.send(msg)?;
    } else if resources.is_empty() {
        eprintln!("Could not find any matching torrents for {}", torrent);
    } else {
        eprintln!(
            "Ambiguous results searching for {}. Potential alternatives include: ",
            torrent
        );
        for res in resources.into_iter().take(3) {
            if let Resource::Torrent(t) = res {
                eprintln!("{}", t.name);
            }
        }
    }
    Ok(())
}

pub fn dl(mut c: Client, url: &str, name: &str) -> Result<()> {
    let resources = search_torrent_name(&mut c, name)?;
    let files = if resources.len() == 1 {
        let msg = CMessage::FilterSubscribe {
            serial: c.next_serial(),
            kind: ResourceKind::File,
            criteria: vec![
                Criterion {
                    field: "torrent_id".to_owned(),
                    op: Operation::Eq,
                    value: Value::S(resources[0].id().to_owned()),
                },
            ],
        };
        if let SMessage::OResourcesExtant { ids, .. } = c.rr(msg)? {
            get_resources(&mut c, ids)?
        } else {
            bail!("Could not get files for torrent!");
        }
    } else if resources.is_empty() {
        eprintln!("Could not find any matching torrents for {}", name);
        return Ok(());
    } else {
        eprintln!(
            "Ambiguous results searching for {}. Potential alternatives include: ",
            name
        );
        for res in resources.into_iter().take(3) {
            if let Resource::Torrent(t) = res {
                eprintln!("{}", t.name);
            }
        }
        return Ok(());
    };

    for file in files {
        let msg = CMessage::DownloadFile {
            serial: c.next_serial(),
            id: file.id().to_owned(),
        };
        if let SMessage::TransferOffer { token, .. } = c.rr(msg)? {
            let client = HClient::new().chain_err(|| ErrorKind::HTTP)?;
            let mut resp = client
                .get(url)
                .chain_err(|| ErrorKind::HTTP)?
                .header(header::Authorization(header::Bearer { token }))
                .send()
                .chain_err(|| ErrorKind::HTTP)?;
            if let Resource::File(f) = file {
                let p = Path::new(&f.path);
                if let Some(par) = p.parent() {
                    fs::create_dir_all(par).chain_err(|| ErrorKind::FileIO)?;
                }
                let mut f = fs::File::create(p).chain_err(|| ErrorKind::FileIO)?;
                io::copy(&mut resp, &mut f).chain_err(|| ErrorKind::FileIO)?;
            } else {
                bail!("Expected a file resource");
            }
        }
    }
    Ok(())
}

pub fn list(mut c: Client, kind: &str, crit: Vec<Criterion>, output: &str) -> Result<()> {
    let k = match kind {
        "torrent" => ResourceKind::Torrent,
        "tracker" => ResourceKind::Tracker,
        "peer" => ResourceKind::Peer,
        "piece" => ResourceKind::Piece,
        "file" => ResourceKind::File,
        "server" => ResourceKind::Server,
        _ => bail!("Unexpected resource kind {}", kind),
    };
    let results = search(&mut c, k, crit)?;
    if output == "text" {
        let mut table = Table::new();
        match k {
            ResourceKind::Torrent => {
                table.add_row(row!["Name", "Done", "DL", "UL", "DL RT", "UL RT", "Peers"]);
            }
            ResourceKind::Tracker => {
                table.add_row(row!["URL", "Torrent", "Error"]);
            }
            ResourceKind::Peer => {
                table.add_row(row!["IP", "Torrent", "DL RT", "UL RT"]);
            }
            ResourceKind::Piece => {
                table.add_row(row!["Torrent", "DLd", "Avail"]);
            }
            ResourceKind::File => {
                table.add_row(row!["Path", "Torrent", "Done", "Prio", "Avail"]);
            }
            ResourceKind::Server => {
                table.add_row(row!["DL RT", "UL RT"]);
            }
        }

        #[cfg_attr(rustfmt, rustfmt_skip)]
        for res in results {
            match k {
                ResourceKind::Torrent => {
                    let t = res.as_torrent();
                    table.add_row(row![
                        t.name,
                        format!("{:.2}%", t.progress * 100.),
                        fmt_bytes(t.transferred_down as f64),
                        fmt_bytes(t.transferred_up as f64),
                        fmt_bytes(t.rate_down as f64) + "/s",
                        fmt_bytes(t.rate_up as f64) + "/s",
                        t.peers
                    ]);
                }
                ResourceKind::Tracker => {
                    let t = res.as_tracker();
                    table.add_row(row![
                        t.url,
                        t.torrent_id,
                        t.error.as_ref().map(|s| s.as_str()).unwrap_or("")
                    ]);
                }
                ResourceKind::Peer => {
                    let p = res.as_peer();
                    let rd = fmt_bytes(p.rate_down as f64) + "/s";
                    let ru = fmt_bytes(p.rate_up as f64) + "/s";
                    table.add_row(row![p.ip, p.torrent_id, rd, ru]);
                }
                ResourceKind::Piece => {
                    let p = res.as_piece();
                    table.add_row(row![p.torrent_id, p.downloaded, p.available]);
                }
                ResourceKind::File => {
                    let f = res.as_file();
                    table.add_row(row![
                        f.path,
                        f.torrent_id,
                        format!("{:.2}%", f.progress as f64 * 100.),
                        f.priority,
                        format!("{:.2}%", f.availability as f64 * 100.)
                    ]);
                }
                ResourceKind::Server => {
                    let s = res.as_server();
                    let rd = fmt_bytes(s.rate_down as f64) + "/s";
                    let ru = fmt_bytes(s.rate_up as f64) + "/s";
                    table.add_row(row![rd, ru]);
                }
            }
        }
        table.printstd();
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&results).chain_err(|| {
                ErrorKind::Serialization
            })?
        );
    }
    Ok(())
}

pub fn pause(mut c: Client, torrents: Vec<&str>) -> Result<()> {
    for torrent in torrents {
        pause_torrent(&mut c, torrent)?;
    }
    Ok(())
}

fn pause_torrent(c: &mut Client, torrent: &str) -> Result<()> {
    let resources = search_torrent_name(c, torrent)?;
    if resources.len() == 1 {
        let mut resource = CResourceUpdate::default();
        resource.id = resources[0].id().to_owned();
        resource.status = Some(Status::Paused);
        let msg = CMessage::UpdateResource {
            serial: c.next_serial(),
            resource,
        };
        c.send(msg)?;
    } else if resources.is_empty() {
        eprintln!("Could not find any matching torrents for {}", torrent);
    } else {
        eprintln!(
            "Ambiguous results searching for {}. Potential alternatives include: ",
            torrent
        );
        for res in resources.into_iter().take(3) {
            if let Resource::Torrent(t) = res {
                eprintln!("{}", t.name);
            }
        }
    }
    Ok(())
}

fn search_torrent_name(c: &mut Client, name: &str) -> Result<Vec<Resource>> {
    search(
        c,
        ResourceKind::Torrent,
        vec![
            Criterion {
                field: "name".to_owned(),
                op: Operation::ILike,
                value: Value::S(format!("%{}%", name)),
            },
        ],
    )
}

fn search(c: &mut Client, kind: ResourceKind, criteria: Vec<Criterion>) -> Result<Vec<Resource>> {
    let s = c.next_serial();
    let msg = CMessage::FilterSubscribe {
        serial: s,
        kind,
        criteria,
    };
    if let SMessage::OResourcesExtant { ids, .. } = c.rr(msg)? {
        get_resources(c, ids)
    } else {
        bail!("Failed to receive extant resource list!");
    }
}

fn get_resources(c: &mut Client, ids: Vec<String>) -> Result<Vec<Resource>> {
    let msg = CMessage::Subscribe {
        serial: c.next_serial(),
        ids,
    };
    let resources = if let SMessage::UpdateResources { resources } = c.rr(msg)? {
        resources
    } else {
        bail!("Failed to received torrent resource list!");
    };

    let mut results = Vec::new();
    for r in resources {
        if let SResourceUpdate::OResource(res) = r {
            results.push(res);
        } else {
            bail!("Failed to received full resource!");
        }
    }
    Ok(results)
}

fn fmt_bytes(num: f64) -> String {
    let num = num.abs();
    let units = ["B", "kiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"];
    if num < 1_f64 {
        return format!("{} {}", num, "B");
    }
    let delimiter = 1024_f64;
    let exponent = cmp::min(
        (num.ln() / delimiter.ln()).floor() as i32,
        (units.len() - 1) as i32,
    );
    let pretty_bytes = format!("{:.2}", num / delimiter.powi(exponent))
        .parse::<f64>()
        .unwrap() * 1_f64;
    let unit = units[exponent as usize];
    format!("{} {}", pretty_bytes, unit)
}