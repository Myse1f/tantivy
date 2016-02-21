use core::reader::SegmentReader;
use core::directory::Directory;
use core::directory::SegmentId;
use core::global::DocId;
use core::schema::Document;
use core::directory::Segment;
use core::collector::Collector;
use std::collections::HashMap;
use core::schema::Term;
use core::error::Result;

pub struct Searcher {
    segments: Vec<SegmentReader>,
    segments_idx: HashMap<SegmentId, usize>,
}

#[derive(Debug)]
pub struct DocAddress(pub SegmentId, pub DocId);

impl Searcher {

    pub fn get_doc(&self, doc_address: &DocAddress) -> Document {
        // TODO err
        let DocAddress(ref segment_id, ref doc_id) = *doc_address;
        let segment_ord = self.segments_idx.get(&segment_id).unwrap();
        let segment_reader = &self.segments[segment_ord.clone()];
        segment_reader.get_doc(doc_id)
    }

    fn add_segment(&mut self, segment: Segment) -> Result<()> {
        SegmentReader::open(segment.clone())
            .map(|segment_reader| {
                let segment_ord = self.segments.len();
                self.segments.push(segment_reader);
                self.segments_idx.insert(segment.id(), segment_ord);
            })
    }

    pub fn new() -> Searcher {
        Searcher {
            segments: Vec::new(),
            segments_idx: HashMap::new(),
        }
    }

    pub fn for_directory(directory: Directory) -> Searcher {
        let mut searcher = Searcher::new();
        for segment in directory.segments().into_iter() {
            searcher.add_segment(segment);
        }
        searcher
    }

    pub fn search(&self, terms: &Vec<Term>, collector: &mut Collector) {
        for segment in &self.segments {
            collector.set_segment(&segment);
            let postings = segment.search(terms);
            for doc_id in postings {
                collector.collect(doc_id);
            }
        }
    }

}
