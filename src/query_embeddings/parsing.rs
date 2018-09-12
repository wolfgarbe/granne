use super::*;

use flate2;
use fnv::FnvHashMap;
use memmap;
use parking_lot::Mutex;
use pbr;
use rayon::prelude::*;
use serde_json;
use std::cmp;
use std::fs::{File, read_dir};
use std::io::{BufRead, BufReader, BufWriter, Read};
use std::path::Path;
use types::AngularVector;

pub fn parse_queries_and_save_to_disk(queries_path: &Path, words_path: &Path, output_path: &Path, show_progress: bool) {
    let word_ids: FnvHashMap<_, _> = {
        let word_file = File::open(&words_path).unwrap();
        let word_file = BufReader::new(word_file);

        word_file.lines().enumerate().map(|(i, w)| {
            let w = w.unwrap();
            (serde_json::from_str::<String>(&w).unwrap(), i)
        }).collect()
    };

    let queries = parsing::parse_queries_in_directory_or_file(queries_path, &word_ids, show_progress);

    let file = File::create(&output_path).unwrap();
    let mut file = BufWriter::new(file);

    queries.write(&mut file).expect("Failed to write queries to disk");
}


pub fn compute_query_vectors_and_save_to_disk(queries_path: &Path, word_embeddings_path: &Path, output_path: &Path, show_progress: bool) {
    let word_embeddings = File::open(word_embeddings_path).expect("Could not open word_embeddings file");
    let word_embeddings = unsafe { memmap::Mmap::map(&word_embeddings).unwrap() };

    let queries = File::open(&queries_path).expect("Could not open queries file");
    let queries = unsafe { memmap::Mmap::map(&queries).unwrap() };

    let queries = QueryEmbeddings::load(&word_embeddings, &queries);

    let file = File::create(&output_path).expect("Could not create output file");
    let mut file = BufWriter::new(file);

    let mut progress_bar = if show_progress {
        Some(pbr::ProgressBar::new(queries.len() as u64))
    } else {
        None
    };

    // generate vectors in chunks to limit memory usage
    let num_chunks = 100;
    let chunk_size = (queries.len() + num_chunks - 1) / num_chunks;
    for i in 0..num_chunks {
        let chunk = (i*chunk_size..cmp::min((i+1)*chunk_size, queries.len())).collect::<Vec<_>>();
        let query_vectors: Vec<AngularVector<'static>> = chunk
            .par_iter()
            .map(|&i| queries.at(i)).collect();

        file_io::write(&query_vectors, &mut file).unwrap();
        if let Some(ref mut progress_bar) = progress_bar {
            progress_bar.add(query_vectors.len() as u64);
        }
    }
}


pub fn parse_queries_in_directory_or_file(path: &Path, word_ids: &FnvHashMap<String, usize>, show_progress: bool) -> QueryVec<'static> {
    let parts = if path.is_dir() {
        let mut parts: Vec<_> = read_dir(path)
            .unwrap()
            .map(|p| p.unwrap().path())
            .collect();
        parts.sort();
        parts
    } else {
        vec![path.to_path_buf()]
    };

    let progress_bar = if show_progress {
        println!("Parsing {} part(s)...", parts.len());
        Some(Mutex::new(pbr::ProgressBar::new(parts.len() as u64)))
    } else {
        None
    };

    let query_parts: Vec<QueryVec> = parts.par_iter().map(|part| {
        let query_file = File::open(&part).expect(&format!("Input file: {:?} not found", &part));

        let queries = if part.to_str().unwrap().ends_with(".gz") {
            let query_file = flate2::read::GzDecoder::new(query_file).expect("Not a valid gzip file.");
            parse_file(query_file, &word_ids)
        } else {
            parse_file(query_file, &word_ids)
        };

        if let Some(ref progress_bar) = progress_bar {
            progress_bar.lock().inc();
        }

        queries
    }).collect();

    let mut progress_bar = if let Some(ref progress_bar) = progress_bar {
        progress_bar.lock().finish_println("All parts parsed\n");
        println!("Collecting queries...");

        Some(pbr::ProgressBar::new(parts.len() as u64))
    } else {
        None
    };

    let mut queries = QueryVec::new();
    for query_part in query_parts {
        queries.extend_from_queryvec(&query_part);

        if let Some(ref mut progress_bar) = progress_bar {
            progress_bar.inc();
        }
    }

    if let Some(ref mut progress_bar) = progress_bar {
        progress_bar.finish_println("Queries collected.\n");
    }

    queries
}


fn parse_file<T: Read>(query_file: T, word_ids: &FnvHashMap<String, usize>) -> QueryVec<'static> {

    let query_file = BufReader::new(query_file);

    let mut queries = QueryVec::new();

    for qs in query_file.lines() {
        let mut query_data = Vec::new();

        let qs = serde_json::from_str::<String>(&qs.unwrap()).unwrap();
        let qs = qs.split(':').last().unwrap();

        for word in qs.split_whitespace() {
            if let Some(&id) = word_ids.get(word) {
                query_data.push(id);
            }
        }

        queries.push(&query_data);
    }

    queries
}
