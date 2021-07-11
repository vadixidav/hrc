#[macro_use]
extern crate std;

use hgg::Hgg;
use rand::{Rng, SeedableRng};
use space::{Bits256, Knn};

#[test]
fn random_insertion_stats() {
    let mut hgg: Hgg<Bits256, ()> = Hgg::new().insert_knn(100);

    // Use a PRNG with good statistical properties for generating 64-bit numbers.
    let mut rng = rand_xoshiro::Xoshiro256PlusPlus::seed_from_u64(0);

    // Generate random keys.
    let keys: Vec<Bits256> = (&mut rng)
        .sample_iter::<[u8; 32], _>(rand::distributions::Standard)
        .map(Bits256)
        .take(1 << 10)
        .collect();

    // Insert keys into HGG.
    for (ix, &key) in keys.iter().enumerate() {
        if ix % 1000 == 0 {
            eprintln!("Inserting {}", ix);
        }
        hgg.insert(key, ());
    }

    eprintln!("Histogram: {:?}", hgg.histogram_neighbors());

    for (ix, key) in keys.iter().enumerate() {
        if ix % 100 == 0 {
            eprintln!("Searching {}", ix);
        }
        // Search each key.
        let distance = hgg.knn(key, 5)[0].distance;
        // Make sure that the best result is this key.
        assert_eq!(distance, 0);
    }
}
