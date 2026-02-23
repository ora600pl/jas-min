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
    CPU,
    Unknown,
}

// ─────────────────────────────────────────────────────────────────────────────
// TIME — time stats for DB Time
// ─────────────────────────────────────────────────────────────────────────────
pub const KEY_STATS_TIME: [&str; 24] = [
    // Top-level time
    "db time",
    "cpu used by this session",
    "recursive cpu usage",

    // Wait class rollups
    "user i/o wait time",
    "concurrency wait time",
    "application wait time",
    "cluster wait time",
    "scheduler wait time",
    "non-idle wait time",

    // Parse time
    "parse time elapsed",
    "parse time cpu",
    "hard parse elapsed time",

    // Redo / commit path time
    "redo write time",
    "redo write total time",
    "redo log space wait time",
    "redo synch time",
    "redo synch time (usec)",

    // File I/O service
    "file io service time",
    "file io wait time",
    "effective io time",

    // Context / lifecycle
    "session connect time",
    "process last non-idle time",
    "in call idle wait time",
    "db time of lwts for this session",
];

// ─────────────────────────────────────────────────────────────────────────────
// VOLUME — (bytes)
// ─────────────────────────────────────────────────────────────────────────────
pub const KEY_STATS_VOLUME: [&str; 22] = [
    // SQL*Net bytes
    "bytes sent via sql*net to client",
    "bytes received via sql*net from client",
    "bytes sent via sql*net to dblink",
    "bytes received via sql*net from dblink",

    // Logical cache volume
    "logical read bytes from cache",

    // Physical I/O bytes
    "physical read bytes",
    "physical read total bytes",
    "physical read total bytes optimized",
    "physical write bytes",
    "physical write total bytes",
    "physical write total bytes optimized",

    // Redo / undo / temp
    "redo size",
    "redo size for direct writes",
    "undo change vector size",
    "temp space allocated (bytes)",

    // Flashback
    "flashback log write bytes",

    // Exadata (safe if absent)
    "cell physical io interconnect bytes",
    "cell physical io interconnect bytes returned by smart scan",
    "cell physical io bytes eligible for predicate offload",
    "cell physical io bytes eligible for smart ios",
    "cell physical io bytes saved by storage index",
    "cell physical io bytes saved by columnar cache",
];

// ─────────────────────────────────────────────────────────────────────────────
// COUNTERS — liczniki opisujące zachowanie i obciążenie
// ─────────────────────────────────────────────────────────────────────────────
pub const KEY_STATS_COUNTERS: [&str; 104] = [
    // Session / cursor churn
    "logons cumulative",
    "logons current",
    "user logons cumulative",
    "user logouts cumulative",
    "opened cursors cumulative",
    "opened cursors current",
    "pinned cursors current",
    "session cursor cache hits",
    "cursor reload failures",

    // Parse / execute
    "parse count (total)",
    "parse count (hard)",
    "parse count (failures)",
    "execute count",
    "user calls",
    "recursive calls",

    // Transactions
    "user commits",
    "user rollbacks",
    "transaction rollbacks",

    // Logical I/O
    "session logical reads",
    "db block gets",
    "db block gets from cache",
    "db block gets direct",
    "consistent gets",
    "consistent gets from cache",
    "consistent gets direct",
    "db block changes",
    "consistent changes",
    "cr blocks created",

    // Physical I/O
    "physical reads",
    "physical reads cache",
    "physical reads cache prefetch",
    "physical reads direct",
    "physical reads direct (lob)",
    "physical reads direct temporary tablespace",
    "physical read io requests",
    "physical read total io requests",
    "physical read total multi block requests",
    "physical read requests optimized",

    "physical writes",
    "physical writes from cache",
    "physical writes direct",
    "physical writes direct (lob)",
    "physical writes direct temporary tablespace",
    "physical write io requests",
    "physical write total io requests",
    "physical write total multi block requests",
    "physical write requests optimized",
    "physical writes non checkpoint",

    // Buffer cache pressure
    "free buffer requested",
    "free buffer inspected",
    "dirty buffers inspected",
    "pinned buffers inspected",
    "hot buffers moved to head of lru",

    // DBWR / checkpoint
    "dbwr checkpoint buffers written",
    "dbwr checkpoints",
    "dbwr lru scans",
    "background checkpoints started",
    "background checkpoints completed",

    // Redo operational
    "redo entries",
    "redo buffer allocation retries",
    "redo writes",
    "redo blocks written",
    "redo log space requests",
    "redo synch writes",
    "redo synch polls",
    "redo synch poll writes",
    "flashback log writes",

    // Sort / workarea
    "sorts (memory)",
    "sorts (disk)",
    "sorts (rows)",
    "workarea executions - optimal",
    "workarea executions - onepass",
    "workarea executions - multipass",

    // Enqueue / locking
    "enqueue requests",
    "enqueue releases",
    "enqueue waits",
    "enqueue timeouts",
    "enqueue deadlocks",
    "enqueue conversions",

    // Access paths
    "table scans (short tables)",
    "table scans (long tables)",
    "table scans (direct read)",
    "table fetch by rowid",
    "table fetch continued row",
    "index range scans",
    "index fast full scans (full)",
    "index fast full scans (direct read)",
    "index fetch by key",
    "leaf node splits",
    "branch node splits",

    // SQL*Net latency amplifier
    "sql*net roundtrips to/from client",
    "sql*net roundtrips to/from dblink",

    // RAC / GC (safe if absent)
    "gc cr blocks served",
    "gc cr blocks received",
    "gc cr blocks flushed",
    "gc cr blocks built",
    "gc current blocks served",
    "gc current blocks received",
    "gc current blocks flushed",
    "gc current blocks pinned",
    "gc read waits",

    // Exadata cache signals
    "cell flash cache read hits",
    "physical read flash cache hits",
    "cell ram cache read hits",
];

// ─────────────────────────────────────────────────────────────────────────────
// CPU — statystyki najlepiej tłumaczące zużycie DB CPU
// (do budowy gradientu DB CPU, NIE DB Time)
// ─────────────────────────────────────────────────────────────────────────────
pub const KEY_STATS_CPU: [&str;  47] = [
    // ── CPU time (direct) ────────────────────────────────────────────
    "cpu used by this session",
    "cpu used when call started",
    "recursive cpu usage",
    "ipc cpu used by this session",
    "global enqueue cpu used by this session",
    "cpu used by lwts for this session",

    // ── Call / execution pressure ───────────────────────────────────
    "user calls",
    "execute count",
    "recursive calls",
    "recursive system api invocations",
    "user logons cumulative",
    "user logouts cumulative",

    // ── Parse pressure (pure CPU work) ──────────────────────────────
    "parse count (total)",
    "parse count (hard)",
    "parse count (failures)",
    "parse time cpu",

    // ── Logical I/O (CPU-heavy cache work) ───────────────────────────
    "session logical reads",
    "db block gets",
    "db block gets from cache",
    "db block gets direct",
    "consistent gets",
    "consistent gets from cache",
    "consistent gets direct",
    "consistent gets pin",
    "consistent gets examination",
    "cr blocks created",

    // ── Buffer / cache churn (CPU work, not waits) ───────────────────
    "free buffer inspected",
    "dirty buffers inspected",
    "pinned buffers inspected",
    "hot buffers moved to head of lru",

    // ── Row / block processing intensity ────────────────────────────
    "db block changes",
    "consistent changes",

    // ── Sorts & workareas (CPU-heavy when memory-resident) ───────────
    "sorts (memory)",
    "sorts (rows)",
    "workarea executions - optimal",
    "workarea executions - onepass",

    // ── SQL engine complexity indicators ─────────────────────────────
    "table fetch by rowid",
    "table fetch continued row",
    "index fetch by key",
    "index range scans",
    "index fast full scans (full)",

    // ── PL/SQL & procedural amplification ───────────────────────────
    "plsql execution elapsed time",      // if present in your stats list
    "plsql compilation elapsed time",    // optional, version-dependent

    // ── Cursor & library cache churn (CPU tax) ───────────────────────
    "opened cursors cumulative",
    "opened cursors current",
    "session cursor cache hits",
    "cursor reload failures",
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
    if contains_normalized(&KEY_STATS_CPU, &name) {
        return StatUnitGroup::CPU;
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
    let name = normalize(stat_name);
    if contains_normalized(&KEY_STATS_TIME, &name) {
        return true;
    } else {
        return false;
    }
}

pub fn is_volume_stat(stat_name: &str) -> bool {
    let name = normalize(stat_name);
    if contains_normalized(&KEY_STATS_VOLUME, &name) {
        return true;
    } else {
        return false;
    }
}
pub fn is_counter_stat(stat_name: &str) -> bool {
    let name = normalize(stat_name);
    if contains_normalized(&KEY_STATS_COUNTERS, &name) {
        return true;
    } else {
        return false;
    }
}

pub fn is_cpu_stat(stat_name: &str) -> bool {
    let name = normalize(stat_name);
    if contains_normalized(&KEY_STATS_CPU, &name) {
        return true;
    } else {
        return false;
    }
}

/* =========================================================================================
   Internals
   ========================================================================================= */

fn contains_normalized(list: &[&str], normalized_name: &str) -> bool {
    list.iter().any(|s| normalize(s) == normalized_name)
}

fn normalize(input: &str) -> String {
    let trimmed = input.trim().to_lowercase();
    // let mut out = String::with_capacity(trimmed.len());
    // let mut prev_space = false;

    // for ch in trimmed.chars() {
    //     if ch.is_whitespace() {
    //         if !prev_space {
    //             out.push(' ');
    //             prev_space = true;
    //         }
    //     } else {
    //         out.push(ch);
    //         prev_space = false;
    //     }
    // }
    trimmed
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