pub mod info;
pub mod peer;
pub mod tracker;
pub mod piece_field;

use bencode::BEncode;
use self::peer::Peer;
use self::tracker::Tracker;
use slab::Slab;
use std::{fmt, io};
use mio::Poll;

pub struct Torrent {
    pub info: info::Info,
    peers: Slab<Peer, usize>,
    // tracker: Tracker,
}

impl fmt::Debug for Torrent {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Torrent {{ info: {:?} }}", self.info)
    }
}

impl Torrent {
    pub fn from_bencode(data: BEncode) -> Result<Torrent, &'static str> {
        let info = info::Info::from_bencode(data)?;
        let peers = Slab::with_capacity(32);
        // let tracker = Tracker::new().unwrap();
        Ok(Torrent {
            info: info,
            peers: peers,
            // tracker: tracker,
        })
    }

    pub fn peer_readable(&mut self, peer: usize) -> io::Result<()> {
        let res = self.peers.get_mut(peer).unwrap().readable()?;
        if res.len() > 0 {
            println!("Got message(s): {:?}!", res);
        } else {
            println!("No messages read!");
        }
        Ok(())
    }

    pub fn peer_writable(&mut self, peer: usize) -> io::Result<bool> {
        self.peers.get_mut(peer).unwrap().writable()
    }

    pub fn file_size(&self) -> usize {
        let mut size = 0;
        for file in self.info.files.iter() {
            size += file.length;
        }
        size
    }

    pub fn remove_peer(&mut self, id: usize) {
        self.peers.remove(id);
    }

    pub fn insert_peer(&mut self, peer: Peer) -> Option<usize> {
        self.peers.insert(peer).ok()
    }

    pub fn get_peer(&self, id: usize) -> Option<&Peer> {
        self.peers.get(id)
    }

    pub fn get_peer_mut(&mut self, id: usize) -> Option<&mut Peer> {
        self.peers.get_mut(id)
    }
}
