#!/bin/bash

REP_TIME=14

export NLS_LANG=AMERICAN_AMERICA.AL32UTF8

cp $ORACLE_HOME/rdbms/admin/sprepcon.sql $ORACLE_HOME/rdbms/admin/sprepcon.sql.bak
cp $ORACLE_HOME/rdbms/admin/sprepins.sql $ORACLE_HOME/rdbms/admin/sprepins.sql.bak
sed 's/linesize_fmt = 80/linesize_fmt = 83/g' $ORACLE_HOME/rdbms/admin/sprepcon.sql > sprepcon.sql.tmp
mv sprepcon.sql.tmp $ORACLE_HOME/rdbms/admin/sprepcon.sql
sed 's/col aa format a80/col aa format a83/g' $ORACLE_HOME/rdbms/admin/sprepins.sql > sprepins.sql.tmp
mv sprepins.sql.tmp $ORACLE_HOME/rdbms/admin/sprepins.sql
sed 's/topn\.old_hash_value,10/st\.sql_id,13/g' $ORACLE_HOME/rdbms/admin/sprepins.sql > sprepins.sql.tmp
mv sprepins.sql.tmp $ORACLE_HOME/rdbms/admin/sprepins.sql
sed 's/topn\.old_hash_value, 10/st\.sql_id,13/g' $ORACLE_HOME/rdbms/admin/sprepins.sql > sprepins.sql.tmp
mv sprepins.sql.tmp $ORACLE_HOME/rdbms/admin/sprepins.sql
sed 's/topn\.old_hash_value,11/st\.sql_id,14/g' $ORACLE_HOME/rdbms/admin/sprepins.sql > sprepins.sql.tmp
mv sprepins.sql.tmp $ORACLE_HOME/rdbms/admin/sprepins.sql
sed 's/topn\.module,80/topn\.module,83/g' $ORACLE_HOME/rdbms/admin/sprepins.sql > sprepins.sql.tmp
mv sprepins.sql.tmp $ORACLE_HOME/rdbms/admin/sprepins.sql


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

