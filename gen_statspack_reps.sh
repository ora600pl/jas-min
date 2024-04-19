#!/bin/bash

REP_TIME=30

export NLS_LANG=AMERICAN_AMERICA.AL32UTF8

while IFS=  read -r line
do
   bsnap=`echo $line | cut -d ' ' -f 1`
   esnap=`echo $line | cut -d ' ' -f 2`

sqlplus "/ as sysdba" << !
define report_name=sp_${bsnap}_${esnap}.txt
define begin_snap=${bsnap}
define end_snap=${esnap}
@?/rdbms/admin/spreport
exit
!


done < <(sqlplus -S "/ as sysdba" << !
set pagesize 0
set trimspool on
set feedback off
with v_snaps as
(
select SNAP_ID || ' ' || lead(snap_id, 1) over (order by snap_id) as cmd, lead(snap_id, 1) over (order by snap_id) lsnap
from perfstat.STATS\$SNAPSHOT
where STARTUP_TIME=(select max(startup_time) from perfstat.STATS\$SNAPSHOT)
and   DBID=(select dbid from v\$database)
and   SNAP_TIME >= SYSDATE-${REP_TIME}
)
select cmd
from v_snaps
where lsnap is not null
/
exit
!
)

