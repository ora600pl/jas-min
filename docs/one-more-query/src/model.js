(function(root){
'use strict';
const sum=a=>a.reduce((s,v)=>s+v,0),mean=a=>sum(a)/a.length;
const dot=(a,b)=>sum(a.map((v,j)=>v*b[j]));
function quantile(a,p){const s=a.slice().sort((a,b)=>a-b),r=(s.length-1)*p,i=Math.floor(r);return s[i]+(s[Math.min(i+1,s.length-1)]-s[i])*(r-i);}
function solve(A,b){
 const a=A.map((r,i)=>[...r,b[i]]),n=b.length,steps=[],backSubstitutions=[];
 for(let k=0;k<n;k++){
  let best=k;for(let j=k+1;j<n;j++)if(Math.abs(a[j][k])>Math.abs(a[best][k]))best=j;
  if(Math.abs(a[best][k])<1e-12)throw Error('Singular system');
  const swap=best!==k?[k,best]:null;if(swap)[a[k],a[best]]=[a[best],a[k]];
  for(let j=k+1;j<n;j++){
   const before=a.map(r=>r.slice()),factor=a[j][k]/a[k][k];
   for(let c=k;c<=n;c++)a[j][c]-=factor*a[k][c];
   steps.push({source:k,target:j,factor,before,swap:j===k+1?swap:null,matrix:a.map(r=>r.slice())});
  }
 }
 const x=Array(n).fill(0);
 for(let k=n-1;k>=0;k--){
  const terms=a[k].slice(k+1,n).map((v,j)=>({column:k+1+j,coefficient:v,beta:x[k+1+j],product:v*x[k+1+j]})),known=sum(terms.map(t=>t.product));
  x[k]=(a[k][n]-known)/a[k][k];backSubstitutions.push({row:k,rhs:a[k][n],diagonal:a[k][k],terms,known,beta:x[k]});
 }
 return {beta:x,steps,backSubstitutions};
}
function prepare(D){const dx=D.x.slice(1).map((r,i)=>r.map((v,j)=>v-D.x[i][j])),dy=D.y.slice(1).map((v,i)=>v-D.y[i]),n=dy.length;
 const avg=D.features.map((_,j)=>mean(dx.map(r=>r[j]))),sd=avg.map((m,j)=>Math.sqrt(sum(dx.map(r=>(r[j]-m)**2))/(n-1))),ym=mean(dy),z=dx.map(r=>r.map((v,j)=>sd[j]?(v-avg[j])/sd[j]:0)),yc=dy.map(v=>v-ym);
 const gram=avg.map((_,j)=>avg.map((_,k)=>sum(z.map(r=>r[j]*r[k]))/n)),rhs=avg.map((_,j)=>sum(z.map((r,i)=>r[j]*yc[i]))/n);
 const magnitude=avg.map((_,j)=>{const a=dx.map(r=>Math.abs(r[j]));return {p90:quantile(a,.9),p99:quantile(a,.99),max:Math.max(...a)};});
 return {dx,dy,n,mean:avg,sd,ymean:ym,z,yc,gram,rhs,magnitude};
}
function ridge(P,lambda){if(!(lambda>0))throw Error('Positive lambda required');const A=P.gram.map((r,j)=>r.map((v,k)=>v+(j===k?lambda:0))),sol=solve(A,P.rhs);return {...sol,intercept:P.ymean,lambda,A,rhs:P.rhs};}
function ranking(D,P,fit,metric){return fit.beta.map((beta,j)=>({j,name:D.features[j],beta,raw:P.sd[j]?beta/P.sd[j]:0,magnitude:P.magnitude[j][metric],impact:beta>0&&P.sd[j]?beta/P.sd[j]*P.magnitude[j][metric]:0,missing:D.missingCounts[j]})).sort((a,b)=>b.impact-a.impact||a.name.localeCompare(b.name));}
function handoff(D,P,fit){const rows=ranking(D,P,fit,'p99'),lead=rows.find(r=>r.impact>0)||null;return {model:'ridge',metric:'p99',lambda:fit.lambda,lead,request:lead?{tool:'get_wait_event_sql_contributors',arguments:{analysis_id:'<analysis_id from start_performance_analysis>',project_id:'<selected project_id>',event_name:lead.name,limit:5},status:'proposed template only; replace session/project placeholders before use; no server contacted; no SQL attribution data in this sample'}:null};}
function lambdaFromSlider(logValue){return Math.abs(logValue-Math.log10(.05))<.025?.05:10**logValue;}
function sourceData(D){return {kind:'retained_educational_observations',features:D.features,target:'historical rounded Load Profile / AAS',waitUnit:'summed seconds per observation window, not a wait rate',y:D.y,x:D.x,missingCounts:D.missingCounts,zeroFilledMissingCounts:D.missingCounts,warning:'Upstream missing entries were zero-filled; their positions and actual window durations are unavailable. Rounded target, selected predictors; not causal evidence.',rowMaskAvailable:false,windowDurationsAvailable:false,timestampsAvailable:false,limitations:['Missing entries were replaced by zero upstream; positions are unknown. Do not identify particular zeros as missing or confirmed measured zeros.','Target values are rounded; window durations are unknown. Do not infer durations or convert wait totals to AAS.','Only four selected waits are included; CPU and other potential predictors are omitted.','All four-feature fits inherit these limitations. Association is not cause or recoverable time.']};}
function evidence(D,P,fit){
 const h=handoff(D,P,fit),settings={ridge:{lambda:fit.lambda},elastic:{lambda:D.parameters.enLambda,alpha:D.parameters.enAlpha,alphaConvention:'L1 mixing fraction: lambda*(alpha*sum(abs(beta))+(1-alpha)*sum(beta^2)/2)',targetScaling:'centered and sample-standardized during fitting; exported coefficients converted back to AAS'},huber:{delta:D.parameters.huberDelta,ridgeLambda:D.parameters.ridgeLambda,ridgeLambdaSource:'recorded reconstruction uses the baseline Ridge lambda in each weighted least-squares step'},quantile:{tau:D.parameters.qTau,lambda:D.parameters.qLambda}};
 return {kind:'educational_evidence_packet',scope:{observations:D.y.length,features:4,target:'adjacent change in DB Time / AAS',input:'historical rounded Load Profile; wait seconds per window',time:'ordered observations; timestamps and window durations unavailable'},method:{transform:'adjacent differences; predictor sample standardization (N-1); centered target',ridgeLambda:fit.lambda,handoffModel:'ridge',handoffMetric:'p99',models:'Ridge recomputed locally at current lambda; other fits retained at their own recorded settings',ranking:'max(0, raw coefficient) × P90/P99/max of absolute predictor changes; fitted increase in predicted adjacent AAS change for that predictor movement with the other three fixed, not observed DB Time composition or a predicted tuning benefit'},quality:{missingSourceEntries:D.features.map((name,j)=>({name,zeroFilled:D.missingCounts[j]})),rowMaskAvailable:false,warning:'Historical zero-filled entries are not confirmed measured zeros. All four-feature fits inherit this limitation. No automatic tuning action.'},models:Object.fromEntries(['ridge','elastic','huber','quantile'].map(m=>[m,{...D.fitStatus[m],eligible:m==='ridge'||D.fitStatus[m].converged,source:m==='ridge'?'live browser recomputation':'retained fit',parameters:settings[m],coefficientsRaw:(m==='ridge'?fit:D.models[m]).beta.map((b,j)=>P.sd[j]?b/P.sd[j]:0)}])),entryPoints:ranking(D,P,fit,'p99').map(r=>({event:r.name,scoreAAS:r.impact,missingSourceEntries:r.missing})),limitations:['Lossy model-based selection, not a substitute for raw observations.','Association does not establish cause, recoverable AAS or CPU savings.','Only four selected waits; CPU and other possible predictors omitted. Hypothetical isolated predictor changes need not occur in the observations.','Q95 estimates a conditional quantile of target changes, not SQL latency p95.'],nextRequest:h.request};
}
function budget(mb,bytesPerToken,context){if(![mb,bytesPerToken,context].every(v=>Number.isFinite(v)&&v>0))throw Error('Invalid estimate inputs');const bytes=mb*1e6,tokens=bytes/bytesPerToken,available=context*.8;return {bytes,tokens,available,ratio:tokens/available,lowerBoundChunks:Math.ceil(tokens/available)};}
// Isolated teaching experiments: these never overwrite retained fits or observations.
function elasticToy(c,l1=.8,l2=.4){const beta=Math.abs(c)<=l1?0:Math.sign(c)*(Math.abs(c)-l1)/(1+l2),error=(beta-c)**2/2,penalty=l1*Math.abs(beta)+l2*beta**2/2;return {beta,error,penalty,total:error+penalty};}
function huberPoint(residual,delta){const a=Math.abs(residual);return {squared:a*a/2,loss:a<=delta?a*a/2:delta*(a-delta/2),weight:a<=delta?1:delta/a,pressure:Math.min(a,delta)};}
function huberThreshold(P){const median=quantile(P.dy,.5),mad=quantile(P.dy.map(v=>Math.abs(v-median)),.5);return {median,mad,delta:Math.max(1e-6,1.345*mad)};}
function quantileToy(prediction,routineCount=4){const observations=[...Array(routineCount).fill(10),30],costs=observations.map(y=>y>=prediction?.95*(y-prediction):.05*(prediction-y));return {observations,costs,total:sum(costs)};}
function percentileTrace(values,p){const sorted=values.slice().sort((a,b)=>a-b),index=(sorted.length-1)*p,lo=Math.floor(index),hi=Math.ceil(index),fraction=index-lo;return {count:sorted.length,position:index+1,lowerPosition:lo+1,upperPosition:hi+1,lower:sorted[lo],upper:sorted[hi],fraction,value:sorted[lo]+fraction*(sorted[hi]-sorted[lo])};}
const api={sum,mean,dot,quantile,solve,prepare,ridge,ranking,handoff,sourceData,lambdaFromSlider,evidence,budget,elasticToy,huberPoint,huberThreshold,quantileToy,percentileTrace};
if(typeof module!=='undefined')module.exports=api;root.DistilleryMath=api;
})(typeof globalThis!=='undefined'?globalThis:this);
