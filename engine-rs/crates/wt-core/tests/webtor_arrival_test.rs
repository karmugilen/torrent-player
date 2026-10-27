use wt_core::arrival::{ArrivalModel, ARRIVAL_FEATURES};
use wt_core::{Client, ClientConfig};

#[test]
fn test_arrival_model_learns_from_arrived_blocks() {
    let mut model = ArrivalModel::new();
    let mut x = [0.0; ARRIVAL_FEATURES];
    x[0] = 1.0;
    x[1] = 0.1; // 100 ms RTT
    x[2] = 0.2;
    x[3] = 0.05;
    x[4] = 0.02;
    x[5] = 0.2;
    x[6] = 1.0;
    x[7] = 5.0;

    let initial_prediction = model.net.predict(x);
    assert!(initial_prediction > 0.0);

    // Train on a known delay
    let target = 0.15; // 150 ms
    for _ in 0..500 {
        let est = model.net.predict(x);
        model.on_block_learned(x, est, target);
    }

    let final_prediction = model.net.predict(x);
    let initial_err = (initial_prediction - target).abs();
    let final_err = (final_prediction - target).abs();

    assert!(final_err < initial_err);
    assert!(model.samples >= 500);

    // Test serialization & restore across client
    let cfg = ClientConfig::default();
    let client = Client::new(cfg).unwrap();
    let serialized = client.arrival_state().unwrap();
    // Default has 0 samples
    assert!(serialized.is_empty());

    // Restore state
    let state_json = serde_json::to_vec(&model).unwrap();
    client.set_arrival_state(&state_json).unwrap();
    let stats = client.arrival_stats();
    assert_eq!(stats.samples, 500);

    let saved = client.arrival_state().unwrap();
    assert!(!saved.is_empty());

    // Bad json returns error
    assert!(client.set_arrival_state(b"invalid json").is_err());
}
