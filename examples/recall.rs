extern crate std;

use hrc::Hrc;
use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256PlusPlus;
use serde::Serialize;
use space::{Bits256, Hamming, MetricPoint};
use std::{io::Read, time::Instant};

const HIGHEST_POWER_SEARCH_SPACE: u32 = 21;
const NUM_SEARCH_QUERRIES: usize = 1 << 18;
const HIGHEST_KNN: usize = 32;
const FRESHENS_PER_INSERT: usize = 2;
const TRAINING_PAIRS: usize = 64;

#[derive(Debug, Serialize)]
struct Record {
    recall: f64,
    search_size: usize,
    knn: usize,
    num_queries: usize,
    seconds_per_query: f64,
    queries_per_second: f64,
}

fn retrieve_search_and_train() -> Vec<Hamming<Bits256>> {
    let descriptor_size_bytes = 61;
    let total_descriptors = (1 << HIGHEST_POWER_SEARCH_SPACE) + NUM_SEARCH_QUERRIES;
    let filepath = "akaze";
    // Read in search space.
    eprintln!(
        "Reading {} search space descriptors of size {} bytes from file \"{}\"...",
        total_descriptors, descriptor_size_bytes, filepath
    );
    let mut file = std::fs::File::open(filepath).expect("unable to open file");
    let mut v = vec![0u8; total_descriptors * descriptor_size_bytes];
    file.read_exact(&mut v)
        .expect("unable to read enough descriptors from the file; add more descriptors to file");
    let search_space: Vec<Hamming<Bits256>> = v
        .chunks_exact(descriptor_size_bytes)
        .map(|b| {
            let mut arr = [0; 32];
            for (d, &s) in arr.iter_mut().zip(b) {
                *d = s;
            }
            Hamming(Bits256(arr))
        })
        .collect();
    eprintln!("Done.");

    search_space
}

fn main() {
    let mut hrc: Hrc<Hamming<Bits256>, ()> = Hrc::new().max_cluster_len(5);

    // Generate random keys.
    let keys = retrieve_search_and_train();

    // Create the rng.
    let mut rng = Xoshiro256PlusPlus::seed_from_u64(42);

    let stdout = std::io::stdout();
    let mut csv_out = csv::Writer::from_writer(stdout.lock());
    for pow in 0..=HIGHEST_POWER_SEARCH_SPACE {
        let search_space = if pow == 0 {
            // If the pow is 0, we can't compute the lower bound properly.
            &keys[0..1]
        } else {
            // In all other cases, take the range from the previous to the current pow.
            &keys[1 << (pow - 1)..1 << pow]
        };
        let query_space = &keys[1 << pow..(1 << pow) + NUM_SEARCH_QUERRIES];
        // Insert keys into HRC.
        eprintln!("Inserting keys into HRC size {}", 1 << pow);
        let start_time = Instant::now();
        for &key in search_space {
            // Insert the key.
            let new_node = hrc.insert(key, (), 16);
            // Train connections to the new key.
            for _ in 0..TRAINING_PAIRS {
                hrc.optimize_connection(new_node, rng.gen_range(0..hrc.len()));
            }
            // Freshen the graph.
            for _ in 0..FRESHENS_PER_INSERT {
                if let Some(node) = hrc.freshen() {
                    for _ in 0..TRAINING_PAIRS {
                        hrc.optimize_connection(node, rng.gen_range(0..hrc.len()));
                    }
                }
            }
        }
        let end_time = Instant::now();
        eprintln!(
            "Finished inserting. Speed was {} keys per second",
            search_space.len() as f64 / (end_time - start_time).as_secs_f64()
        );

        eprintln!("Computing correct nearest neighbors for recall calculation");
        let correct_nearest: Vec<Hamming<Bits256>> = query_space
            .iter()
            .map(|query| {
                keys[..1 << pow]
                    .iter()
                    .copied()
                    .min_by_key(|key| query.distance(key))
                    .unwrap()
            })
            .collect();
        eprintln!("Finished computing the correct nearest neighbors");

        for knn in 1..=HIGHEST_KNN {
            eprintln!("doing size {} with knn {}", 1 << pow, knn);
            let start_time = Instant::now();
            let search_bests: Vec<usize> = query_space
                .iter()
                .map(|query| hrc.search_knn_from(0, query, knn)[0].0)
                // .map(|query| hrc.search_from(0, query).0)
                .collect();
            let end_time = Instant::now();
            let num_correct = search_bests
                .iter()
                .zip(correct_nearest.iter())
                .zip(query_space.iter())
                .filter(|&((&searched_ix, correct), query)| {
                    hrc.get_key(searched_ix).unwrap().distance(query) == correct.distance(query)
                })
                .count();
            let recall = num_correct as f64 / NUM_SEARCH_QUERRIES as f64;
            let seconds_per_query = (end_time - start_time)
                .div_f64(NUM_SEARCH_QUERRIES as f64)
                .as_secs_f64();
            let queries_per_second = seconds_per_query.recip();
            csv_out
                .serialize(Record {
                    recall,
                    search_size: 1 << pow,
                    knn,
                    num_queries: NUM_SEARCH_QUERRIES,
                    seconds_per_query,
                    queries_per_second,
                })
                .expect("failed to serialize record");
            csv_out.flush().expect("failed to flush record");
            eprintln!("finished size {} with knn {}", 1 << pow, knn);
        }
    }
}
