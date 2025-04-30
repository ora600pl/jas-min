use std::collections::{HashMap, HashSet, BTreeMap};
use crate::awr::{WaitEvents, HostCPU, LoadProfile, SQLCPUTime, SQLIOTime, SQLGets, SQLReads, AWR, AWRSCollection};


fn median(data: &[f64]) -> f64 {
    let mut sorted = data.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let len = sorted.len();
    if len == 0 {
        return 0.0;
    }
    if len % 2 == 0 {
        (sorted[len / 2 - 1] + sorted[len / 2]) / 2.0
    } else {
        sorted[len / 2]
    }
}

fn mad(data: &[f64], med: f64) -> f64 {
    let deviations: Vec<f64> = data.iter().map(|x| (x - med).abs()).collect();
    median(&deviations)
}

fn get_event_map_vectors(awrs: &Vec<AWR>) -> HashMap<String, Vec<f64>> {
    //Create list of all events
    let all_events: HashSet<String> = awrs
                                    .iter()
                                    .flat_map(|awr| awr.foreground_wait_events.iter())
                                    .map(|e| e.event.clone())
                                    .collect();

    //This will hold event name and vector of values filled with 0.0 as default value
    let mut event_map: HashMap<String, Vec<f64>> = all_events
                                                    .iter()
                                                    .map(|e| (e.clone(), vec![0.0; awrs.len()]))
                                                    .collect();

    //we are iterating over AWR
    for (i, awr) in awrs.iter().enumerate() {
        //Let's create a HashMap from all foreground and background events
        let snapshot_map: HashMap<&String, f64> = awr
                                                .foreground_wait_events
                                                .iter()
                                                .map(|e| (&e.event, e.total_wait_time_s))
                                                .collect();

        //Let's go through all of the event names
        for event in &all_events {
            //If some event name exists in this snapshot, set actual value in the map, instead of 0.0
            if let Some(&val) = snapshot_map.get(event) {
                event_map.get_mut(event).unwrap()[i] = val;
            }
        }
    }
    event_map
}

//Median Absolute Deviation for anomalies detection in wait events
pub fn detect_event_anomalies_mad(awrs: &Vec<AWR>) -> BTreeMap<usize, Vec<(f64,String)>> {
    let mut anomalies: BTreeMap<usize, Vec<(f64,String)>> = BTreeMap::new();
    let event_map_vectors = get_event_map_vectors(awrs);
    
    let threshold = 7.0; //Default threshold for MAD

    for (event, values) in &event_map_vectors {
        let med = median(values);
        let mad_val = mad(values, med);

        if mad_val == 0.0 {
            continue; // no nomalies - just move on
        }

        for (i, &val) in values.iter().enumerate() {
            //if anomaly is bigger than threshold - put event name on index corresponding to detected anomaly
            let val_mad_check = ((val - med).abs()) / mad_val ;
            if val_mad_check > threshold { 
                anomalies.entry(i).or_default().push((val_mad_check, event.clone()));
            }
        }
    }

    anomalies
}
