select lp.statname, corr(dbt.per_second, lp.per_second) as crr
from PERF_LOAD_PROFILE dbt, PERF_LOAD_PROFILE lp
where dbt.AUDIT_ID=lp.audit_id 
and   dbt.begin_snap=lp.begin_snap 
and   dbt.statname = 'DB time(s):'
and   lp.statname != 'DB time(s):'
group by lp.statname 
order by crr desc nulls last;

select lp.statname, corr(dbt.per_second, lp.total_wait_time) as crr, 
                    sum(lp.total_wait_time), 
                    avg(dbt.PER_SECOND),
                    avg(lp.total_wait_time)/1000, 
                    stddev(lp.total_Wait_time),
                    sum(dbt.per_second)
from PERF_LOAD_PROFILE dbt, PERF_FORGROUND_EVENTS lp
where dbt.AUDIT_ID=lp.audit_id 
and   dbt.begin_snap=lp.begin_snap 
and   dbt.statname = 'DB time(s):'
and   lp.statname != 'DB time(s):'
group by lp.statname 
having corr(dbt.per_second, lp.total_wait_time) >= 0.2
order by avg(lp.total_wait_time) desc, crr desc nulls last;


select lp.statname, corr(dbt.per_second, lp.total) as crr, 
                    sum(lp.total), 
                    avg(lp.total), 
                    stddev(lp.total),
                    avg(dbt.per_second)
from PERF_LOAD_PROFILE dbt, PERF_INSTANCE_ACTIVITY lp
where dbt.AUDIT_ID=lp.audit_id 
and   dbt.begin_snap=lp.begin_snap 
and   dbt.statname = 'DB time(s):'
and   lp.statname != 'DB time(s):'
group by lp.statname 
having corr(dbt.per_second, lp.total) >= 0.5
order by crr desc nulls last;

select *
from PERF_INSTANCE_ACTIVITY;

begin 
  PKG_AUDIT.P_CLEAR_DATA;
end;
/

select distinct sqlid, crr
from V_TOP10_SQLS_ELA_BY_CORR
order by crr desc;

select round(count(1) over ()/720, 2), 
      round(avg(ela_by_exec) over (),2),
      round(stddev(ela_by_exec) over (),2),
      max(ela_by_exec) over (),
      min(ela_by_exec) over (),
      round(avg(executions) over (),2),
      round(stddev(executions) over (),2),
      max(executions) over (),
      min(executions) over (),
       s.*
from SQL_BY_ELA s
where SQLID='3808994653';

select count(distinct BEGIN_SNAP)
from SQL_BY_ELA;

begin
    pkg_audit.set_context('prdehsfa', 4);
end;
/

select distinct audit_id from PERF_FORGROUND_EVENTS;

with v_commits as (
select sum(total) as commits, BEGIN_SNAP
from PERF_INSTANCE_ACTIVITY
where statname in ('user commits', 'user rollbacks')
group by BEGIN_SNAP)
select round(avg(uc.TOTAL/c.commits),2), round(stddev(uc.TOTAL/c.commits), 2)
from v_commits c, PERF_INSTANCE_ACTIVITY uc
where c.begin_snap=uc.BEGIN_SNAP
and   uc.STATNAME='user calls';

alter session set nls_date_format='YYYY-MM-DD:HH24:MI:SS';

select *
from PERF_FORGROUND_EVENTS;

with v_corr as (
select sqlid, p.TOTAL_WAIT_TIME, s.elapsed_time, s.begin_snap,
       corr(p.TOTAL_WAIT_TIME, s.elapsed_time) over (partition by s.sqlid) as crr
from PERF_FORGROUND_EVENTS p, V_FILLED_SQL_BY_ELA2 s 
where p.BEGIN_SNAP=s.BEGIN_SNAP
and p.STATNAME in ('db file sequential read')
order by crr desc nulls last
)
select distinct sqlid, crr
from v_corr 
where crr>=0.45;