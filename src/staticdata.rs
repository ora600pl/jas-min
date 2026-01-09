// Goal:
// - Split V$SYSSTAT metrics into 3 groups for 3 separate gradients:
//   1) time-based
//   2) volume-based (bytes/blocks-like)
//   3) count-based (counters)
//
// Matching approach:
// - Normalize strings (trim + lowercase + collapse spaces)
// - Prefer explicit constant lists for known important metrics
// - Use simple heuristics as a fallback (suffix/pattern)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatUnitGroup {
    Time,
    Volume,
    Counter,
    Unknown,
}

/// TIME group: values represent time spent (elapsed time, wait time, write time, CPU time, etc.)
pub const KEY_STATS_TIME: [&str; 12] = [
    "application wait time",
    "cluster wait time",
    "concurrency wait time",
    "file io wait time",
    "user i/o wait time",

    "parse time elapsed",
    "parse time cpu",
    "hard parse elapsed time",

    "cpu used by this session",
    "recursive cpu usage",

    "redo write time",
    "redo log space wait time",
];


/// VOLUME group: bytes/blocks/size-like.
pub const KEY_STATS_VOLUME: [&str; 11] = [
    "bytes received via sql*net from client",
    "bytes sent via sql*net to client",
    "bytes received via sql*net from dblink",
    "bytes sent via sql*net to dblink",

    "physical read bytes",
    "physical read total bytes",
    "physical write bytes",
    "physical write total bytes",

    "redo size",
    "redo size for lost write detection",

    "flashback log write bytes"
];

/// COUNTER group: counts of operations/events.
pub const KEY_STATS_COUNTERS: [&str; 47] = [
    // Logon storm / session churn  
    "logons cumulative",
    "logons current",

    // Cursor churn  
    "opened cursors cumulative",
    "opened cursors current",

    // Parse storm indicators  
    "parse count (describe)",
    "parse count (hard)",
    "parse count (total)",
    "execute count",

    // “User calls” is great for spotting chatty apps / bad call patterns  
    "user calls",
    "recursive calls",

    // Commit/rollback policy signals  
    "user commits",
    "user rollbacks",

    // Commit/cleanout/rollback mechanics (more meaningful than raw "user rollbacks")  
    "commit wait requested",
    "commit cleanouts",
    "commit cleanouts successfully completed",
    "commit cleanout failures: block lost",
    "cleanouts and rollbacks - consistent read gets",
    "cleanouts only - consistent read gets",
    "rollbacks only - consistent read gets",
    "transaction tables consistent read rollbacks",
    "rollback changes - undo records applied",
    "transaction tables consistent reads - undo records applied",

    // Buffer/cache pressure proxies  
    "free buffer inspected",
    "free buffer requested",
    //"db block gets",
    //"db block gets direct",
    //"db block gets from cache",
    //"consistent gets",
    //"consistent gets direct",
    //"consistent gets from cache",
    //"session logical reads",

    // Disk I/O request shape (efficiency signals)  
    "physical read io requests",
    "physical read total io requests",
    "physical read total multi block requests",
    "physical read requests optimized",
    "physical write io requests",
    "physical write total io requests",
    "physical write total multi block requests",

    // Direct path reads (bypass buffer cache)  
    "physical reads",
    "physical reads cache",
    "physical reads cache prefetch",
    "physical reads direct",
    "physical reads direct (lob)",
    "physical reads direct temporary tablespace",

    // Sort/workarea spill signals  
    "sorts (disk)",

    // Redo/space pressure  
    "redo log space requests",
    "redo synch writes",
    "redo writes",
    "redo wastage",

    // Enqueue activity (locks)  
    "enqueue requests",
    "enqueue releases",
    "enqueue waits",
    "enqueue timeouts",

    // Index split noise can be a symptom of hot indexes / bad fillfactor (situational)  
    "branch node splits",
];

pub fn classify_stat_unit_group(stat_name: &str) -> StatUnitGroup {
    let name = normalize(stat_name);

    if contains_normalized(&KEY_STATS_TIME, &name) {
        return StatUnitGroup::Time;
    }
    if contains_normalized(&KEY_STATS_VOLUME, &name) {
        return StatUnitGroup::Volume;
    }
    if contains_normalized(&KEY_STATS_COUNTERS, &name) {
        return StatUnitGroup::Counter;
    }

    // Heuristic fallback (kept conservative)
    // if name.ends_with(" wait time")
    //     || name.ends_with(" elapsed time")
    //     || name.ends_with(" write time")
    //     || name.contains(" time ")
    //     || name.ends_with(" time")
    // {
    //     return StatUnitGroup::Time;
    // }

    // if name.contains(" bytes") || name.ends_with(" bytes") || name.ends_with(" size") {
    //     return StatUnitGroup::Volume;
    // }

    // // Most SYSSTAT entries are counters if not explicitly time/volume.
    // StatUnitGroup::Counter
    StatUnitGroup::Unknown
}

pub fn is_time_stat(stat_name: &str) -> bool {
    classify_stat_unit_group(stat_name) == StatUnitGroup::Time
}
pub fn is_volume_stat(stat_name: &str) -> bool {
    classify_stat_unit_group(stat_name) == StatUnitGroup::Volume
}
pub fn is_counter_stat(stat_name: &str) -> bool {
    classify_stat_unit_group(stat_name) == StatUnitGroup::Counter
}

/* =========================================================================================
   Internals
   ========================================================================================= */

fn contains_normalized(list: &[&str], normalized_name: &str) -> bool {
    list.iter().any(|s| normalize(s) == normalized_name)
}

fn normalize(input: &str) -> String {
    let trimmed = input.trim().to_lowercase();
    let mut out = String::with_capacity(trimmed.len());
    let mut prev_space = false;

    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

const IDLE:  [&str;172] = ["cached session",                                              
"VKTM Logical Idle Wait",                                      
"VKTM Init Wait for GSGA",                                     
"IORM Scheduler Slave Idle Wait",                              
"rdbms ipc message",                                           
"i/o slave wait",                                              
"OFS Receive Queue",                                           
"OFS idle",                                                    
"Generic Process Pool Dispatcher: idle",                       
"Generic Process Pool Worker: sleep",                          
"VKRM Idle",                                                   
"wait for unread message on broadcast channel",                
"wait for unread message on multiple broadcast channels",      
"class slave wait",                                            
"idle class spare wait event 1",                               
"idle class spare wait event 2",                               
"idle class spare wait event 3",                               
"idle class spare wait event 4",                               
"idle class spare wait event 5",                               
"idle class spare wait event 6",                               
"idle class spare wait event 7",                               
"idle class spare wait event 8",                               
"idle class spare wait event 9",                               
"idle class spare wait event 10",                              
"RMA: IPC0 completion sync",                                   
"PING",                                                        
"spawn request deferred",                                      
"watchdog main loop",                                          
"process in prespawned state",                                 
"pmon timer",                                                  
"pman timer",                                                  
"DNFS disp IO slave idle",                                     
"NVM disp IO slave idle",                                      
"BRDG: bridge controller idle",                                
"Network Retrans by Server",                                   
"Network Retrans by Client",                                   
"Distributed Trace: Archival Worker Idle",                     
"DIAG idle wait",                                              
"ges remote message",                                          
"SCM slave idle",                                              
"LMS CR slave timer",                                          
"gcs remote message",                                          
"gcs yield cpu",                                               
"heartbeat monitor sleep",                                     
"GCR sleep",                                                   
"Shutdown completion due to error",                            
"SGA: MMAN sleep for component shrink",                        
"DBWR timer",                                                  
"Data Guard: Gap Manager",                                     
"Data Guard: controlfile update",                              
"MRP redo arrival",                                            
"Data Guard: Timer",                                           
"LNS ASYNC archive log",                                       
"LNS ASYNC dest activation",                                   
"LNS ASYNC end of log",                                        
"Archiver: redo logs",                                         
"simulated log write delay",                                   
"heartbeat redo informer",                                     
"LGWR real time apply sync",                                   
"LGWR worker group idle",                                      
"parallel recovery slave idle wait",                           
"Backup Appliance waiting for work",                           
"Backup Appliance waiting restore start",                      
"Backup Appliance Surrogate wait",                             
"Backup Appliance Servlet wait",                               
"Backup Appliance Comm SGA setup wait",                        
"LogMiner builder: idle",                                      
"LogMiner builder: branch",                                    
"LogMiner preparer: idle",                                     
"LogMiner reader: log (idle)",                                 
"LogMiner reader: redo (idle)",                                
"LogMiner merger: idle",                                       
"LogMiner client: transaction",                                
"LogMiner: other",                                             
"LogMiner: activate",                                          
"LogMiner: reset",                                             
"LogMiner: find session",                                      
"LogMiner: internal",                                          
"Logical Standby Apply Delay",                                 
"parallel recovery coordinator waits for slave cleanup",       
"parallel recovery coordinator idle wait",                     
"parallel recovery control message reply",                     
"parallel recovery slave next change",                         
"nologging fetch slave idle",                                  
"recovery sender idle",                                        
"recovery receiver idle",                                      
"recovery coordinator idle",                                   
"recovery logmerger idle",                                     
"block compare coord process idle",                            
"Data Guard PDB query SCN service idle",                       
"True Cache: background process idle",                         
"PX Deq: Txn Recovery Start",                                  
"PX Deq: Txn Recovery Reply",                                  
"fbar timer",                                                  
"smon timer",                                                  
"PX Deq: Metadata Update",                                     
"Space Manager: slave idle wait",                              
"PX Deq: Index Merge Reply",                                   
"PX Deq: Index Merge Execute",                                 
"PX Deq: Index Merge Close",                                   
"PX Deq: kdcph_mai",                                           
"PX Deq: kdcphc_ack",                                          
"imco timer",                                                  
"IMFS defer writes scheduler",                                 
"memoptimize write drain idle",                                
"MLE sleep",                                                   
"virtual circuit next request",                                
"shared server idle wait",                                     
"dispatcher timer",                                            
"cmon timer",                                                  
"pool server timer",                                           
"lreg timer",                                                  
"JOX Jit Process Sleep",                                       
"jobq slave wait",                                             
"pipe get",                                                    
"PX Deque wait",                                               
"PX Idle Wait",                                                
"PX Deq Credit: need buffer",                                  
"PX Deq Credit: send blkd",                                    
"PX Deq: Msg Fragment",                                        
"PX Deq: Parse Reply",                                         
"PX Deq: Execute Reply",                                       
"PX Deq: Execution Msg",                                       
"PX Deq: Table Q Normal",                                      
"PX Deq: Table Q Sample",                                      
"REPL Apply: txns",                                            
"REPL Capture/Apply: messages",                                
"REPL Capture: archive log",                                   
"single-task message",                                         
"SQL*Net message from client",                                 
"SQL*Net vector message from client",                          
"SQL*Net vector message from dblink",                          
"PL/SQL lock timer",                                           
"Streams AQ: emn coordinator idle wait",                       
"EMON slave idle wait",                                        
"Emon coordinator main loop",                                  
"Emon slave main loop",                                        
"Streams AQ: waiting for messages in the queue",               
"Streams AQ: waiting for time management or cleanup tasks",    
"Streams AQ: delete acknowledged messages",                    
"Streams AQ: deallocate messages from Streams Pool",           
"Streams AQ: qmn coordinator idle wait",                       
"Streams AQ: qmn slave idle wait",                             
"AQ: 12c message cache init wait",                             
"AQ Cross Master idle",                                        
"AQPC idle",                                                   
"Streams AQ: load balancer idle",                              
"Sharded  Queues : Part Maintenance idle",                     
"Sharded  Queues : Part Truncate idle",                        
"REPL Capture/Apply: RAC AQ qmn coordinator",                  
"Streams AQ: opt idle",                                        
"HS message to agent",                                         
"ASM background timer",                                        
"ASM cluster membership changes",                              
"AUTO access ASM_CLIENT registration",                         
"iowp msg",                                                    
"iowp file id",                                                
"netp network",                                                
"gopp msg",                                                    
"auto-sqltune: wait graph update",                             
"WCR: replay client notify",                                   
"WCR: replay clock",                                           
"WCR: replay paused",                                          
"JS external job",                                             
"cell worker idle",                                            
"Multi-Tenant Redo File Server - Flush Header Interval",       
"Sharding replication",                                        
"Consensus service idle",                                      
"Blockchain apply clean",                                      
"blockchain apply short",                                      
"blockchain apply long",                                       
"Blockchain reader process idle"];

pub fn is_idle(event_name: &str) -> bool {
    for event in IDLE {
        if event.starts_with(event_name) {
            return true;
        }
    }
    false
}