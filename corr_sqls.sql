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
having corr(dbt.per_second, lp.total_wait_time) >= 0.5
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
from V_TOP10_SQLS_ELA_BY_CORR;

begin
    pkg_audit.set_context('szpital2', 4);
end;
/