/* Educational numerical reconstruction of src/gradient.rs and src/quantile.rs.
 * No I/O, random numbers, data provenance, or production-runtime claims.
 * Keep matrix column order aligned with the exported Oracle example.
 */
'use strict';
const M=require('../src/math.js');
const {dot,mean,norm,normalSystem,solve,standardize,soft,pinball}=M;
const sum=a=>a.reduce((s,v)=>s+v,0),zeros=n=>Array(n).fill(0);
const median=a=>{const s=a.slice().sort((a,b)=>a-b),m=s.length>>1;return s.length%2?s[m]:(s[m-1]+s[m])/2;};
const mad=a=>{const m=median(a);return median(a.map(v=>Math.abs(v-m)));};
const percentile=(a,q)=>{const s=a.map(Math.abs).sort((a,b)=>a-b),r=q*(s.length-1),i=Math.floor(r),f=r-i;return s[i]*(1-f)+s[Math.min(i+1,s.length-1)]*f;};
const columns=X=>X[0].map((_,j)=>X.map(r=>r[j]));
const xtv=(X,v)=>X[0].map((_,j)=>sum(X.map((r,i)=>r[j]*v[i])));
const mv=(A,b)=>A.map(r=>dot(r,b));
const sq=v=>dot(v,v);
function cholesky(A){const n=A.length,L=A.map(()=>zeros(n));for(let i=0;i<n;i++)for(let j=0;j<=i;j++){let s=A[i][j];for(let k=0;k<j;k++)s-=L[i][k]*L[j][k];if(i===j){if(s<=0)throw Error('Matrix is not positive definite');L[i][j]=Math.sqrt(s);}else L[i][j]=s/L[j][j];}return L;}
function cholSolve(L,b){const n=b.length,y=zeros(n),x=zeros(n);for(let i=0;i<n;i++){let v=b[i];for(let j=0;j<i;j++)v-=L[i][j]*y[j];y[i]=v/L[i][i];}for(let i=n-1;i>=0;i--){let v=y[i];for(let j=i+1;j<n;j++)v-=L[j][i]*x[j];x[i]=v/L[i][i];}return x;}
function elastic(X,y,lambda,alpha,initial=null,trace=false,focus=[]){const n=y.length,p=X[0].length,b=initial?initial.slice():zeros(p),r=y.map((v,i)=>v-dot(X[i],b)),cs=columns(X),a=cs.map(c=>sq(c)/n||1e-12),states=[];let iterations=0,change=0;
 for(let k=1;k<=5000;k++){change=0;for(let j=0;j<p;j++){const old=b[j];for(let i=0;i<n;i++)r[i]+=cs[j][i]*old;const c=dot(cs[j],r)/n,threshold=lambda*alpha,denominator=a[j]+lambda*(1-alpha);b[j]=soft(c,threshold)/denominator;for(let i=0;i<n;i++)r[i]-=cs[j][i]*b[j];change=Math.max(change,Math.abs(b[j]-old));if(trace&&(k<=10||k%50===0))states.push({k,j,old,c,threshold,denominator,a:a[j],beta:b.slice(),row:focus.map(i=>({partial:r[i]+cs[j][i]*b[j],residual:r[i]})),objective:sq(r)/(2*n)+lambda*(alpha*sum(b.map(Math.abs))+(1-alpha)*sq(b)/2)});}iterations=k;if(change<1e-6)break;}
 if(trace)states.push({k:iterations,done:true,beta:b.slice(),change,objective:sq(r)/(2*n)+lambda*(alpha*sum(b.map(Math.abs))+(1-alpha)*sq(b)/2)});
 return {beta:b,iterations,change,states};
}
function cv(dx,dy,alpha){const n=dy.length,initial=Math.max(4,Math.floor(n/3)),ratios=Array.from({length:40},(_,i)=>Math.exp(Math.log(.001)*i/39)),folds=[],losses=ratios.map(()=>[]);
 for(let f=0;f<5;f++){const start=initial+Math.floor((n-initial)*f/5),end=initial+Math.floor((n-initial)*(f+1)/5),stats=columns(dx.slice(0,start)).map(standardize),ys=standardize(dy.slice(0,start)),X=dx.slice(0,start).map(r=>r.map((v,j)=>stats[j].sd?(v-stats[j].mean)/stats[j].sd:0)),y=dy.slice(0,start).map(v=>(v-ys.mean)/ys.sd),V=dx.slice(start,end).map(r=>r.map((v,j)=>stats[j].sd?(v-stats[j].mean)/stats[j].sd:0)),vy=dy.slice(start,end).map(v=>(v-ys.mean)/ys.sd),max=Math.max(...columns(X).map(c=>Math.abs(dot(c,y)/start)))/alpha;let warm=null;const foldLoss=[];
  for(let j=0;j<ratios.length;j++){const fit=elastic(X,y,max*ratios[j],alpha,warm);warm=fit.beta;const loss=mean(vy.map((v,i)=>(v-dot(V[i],warm))**2));losses[j].push(loss);foldLoss.push(loss);}folds.push({start,end,xmean:stats.map(s=>s.mean),xstd:stats.map(s=>s.sd),ymean:ys.mean,ystd:ys.sd,lambdaMax:max,losses:foldLoss});
 }
 const means=losses.map(mean),se=losses.map(a=>standardize(a).sd/Math.sqrt(a.length)),best=means.indexOf(Math.min(...means)),limit=means[best]+se[best],selected=means.findIndex(v=>v<=limit);return {folds,ratios,means,se,best,limit,selected};
}
function quantile(X,raw,{lambda=.0005,tau=.95,tol=1e-6,max=20000,focus=[]}={}){
 const n=raw.length,p=X[0].length,ymean=mean(raw),scale=Math.sqrt(mean(raw.map(v=>(v-ymean)**2)))||1,y=raw.map(v=>(v-ymean)/scale),A=X.map(r=>[1,...r]),G=normalSystem(A,zeros(n)).A.map(r=>r.map(v=>v*n));let rho=1,theta=zeros(p+1),r=y.slice(),u=zeros(n),converged=false,refactors=1;
 const factor=()=>cholesky(G.map((r,j)=>r.map((v,k)=>v+(j===k&&j>0?n*lambda/rho:0))));let L=factor(),last,states=[];
 function bounds(){const alpha=u.map(v=>-rho*v);let lo=Math.min(...alpha)-tau,hi=Math.max(...alpha)-(tau-1);for(let k=0;k<80;k++){const shift=(lo+hi)/2,s=sum(alpha.map(v=>Math.max(tau-1,Math.min(tau,v-shift))));if(s>0)lo=shift;else hi=shift;}const feasible=alpha.map(v=>Math.max(tau-1,Math.min(tau,v-(lo+hi)/2))),moment=xtv(A,feasible).map(v=>v/n),lower=dot(y,feasible)/n-sq(moment.slice(1))/(2*lambda),res=y.map((v,i)=>v-dot(A[i],theta)),upper=mean(res.map(v=>pinball(v,tau)))+lambda*sq(theta.slice(1))/2;return {upper,lower,gap:Math.max(0,upper-lower),dualSum:sum(feasible),dualMin:Math.min(...feasible),dualMax:Math.max(...feasible)};}
 for(let k=1;k<=max;k++){
  const oldR=r.slice(),oldU=u.slice(),right=xtv(A,y.map((v,i)=>v-r[i]-u[i]));theta=cholSolve(L,right);const prediction=mv(A,theta),v=y.map((yi,i)=>yi-prediction[i]-u[i]);r=v.map(v=>v>tau/rho?v-tau/rho:v<(tau-1)/rho?v-(tau-1)/rho:0);const mismatch=y.map((yi,i)=>prediction[i]+r[i]-yi);u=u.map((ui,i)=>ui+mismatch[i]);const primal=norm(mismatch),dual=rho*norm(xtv(A,r.map((v,i)=>v-oldR[i]))),ptol=tol*(Math.sqrt(n)+Math.max(norm(prediction),norm(r),norm(y))),dtol=tol*(Math.sqrt(p+1)+rho*norm(xtv(A,u))),residualsPass=primal<=ptol&&dual<=dtol;
  let cert=null;if(residualsPass){cert=bounds();converged=cert.gap<=tol*(1+Math.abs(cert.upper));}
  const save=k<=10||k%100===0||converged||k===max;
  if(save){cert=cert||bounds();last={k,rho,theta:theta.slice(),right,primal,dual,ptol,dtol,...cert,converged,rows:focus.map(i=>({y:y[i],pred:prediction[i],v:v[i],oldR:oldR[i],r:r[i],oldU:oldU[i],u:u[i],mismatch:mismatch[i]}))};states.push(last);}
  if(converged)break;
  if(k%50===0){const next=primal>10*dual?Math.min(1024,rho*2):dual>10*primal?Math.max(.0001,rho/2):rho;if(next!==rho){u=u.map(v=>v*rho/next);rho=next;L=factor();refactors++;}}
 }
 return {beta:theta.slice(1).map(v=>v*scale),intercept:ymean+theta[0]*scale,scale,ymean,lambda,tau,tol,converged,iterations:last.k,states,gram:G,refactors,final:last};
}
function prepare(D){const n=D.y.length-1,dx=D.x.slice(1).map((r,i)=>r.map((v,j)=>v-D.x[i][j])),dy=D.y.slice(1).map((v,i)=>v-D.y[i]),stats=columns(dx).map(standardize),ymean=mean(dy),yc=dy.map(v=>v-ymean),ys=standardize(dy),z=dx.map(r=>r.map((v,j)=>stats[j].sd?(v-stats[j].mean)/stats[j].sd:0)),G=normalSystem(z,yc,0),ridgeSystem=normalSystem(z,yc,D.meta.ridge_lambda),focus=D.focus;
 M.setLanguage('en');const gaussEN=M.gaussian(ridgeSystem.A,ridgeSystem.b);M.setLanguage('pl');const gaussPL=M.gaussian(ridgeSystem.A,ridgeSystem.b);M.setLanguage('en');
 const crossValidation=cv(dx,dy,D.meta.en_alpha),target=yc.map(v=>v/ys.sd),lambdaMax=Math.max(...columns(z).map(c=>Math.abs(dot(c,target)/n)))/D.meta.en_alpha,lambda=lambdaMax*crossValidation.ratios[crossValidation.selected],en=elastic(z,target,lambda,D.meta.en_alpha,null,true,focus);en.scaledBeta=en.beta.slice();en.beta=en.beta.map(v=>v*ys.sd);en.lambda=lambda;en.lambdaMax=lambdaMax;en.scale=ys.sd;en.alpha=D.meta.en_alpha;
 const delta=Math.max(1e-6,1.345*mad(yc)),hstates=[];let hb=zeros(4),change=0;
 for(let k=1;k<=100;k++){const before=hb.slice(),res=yc.map((v,i)=>v-dot(z[i],hb)),weights=res.map(v=>M.huberWeight(v,delta)),A=Array.from({length:4},()=>zeros(4)),b=zeros(4);for(let i=0;i<n;i++)for(let j=0;j<4;j++){b[j]+=z[i][j]*weights[i]*yc[i]/n;for(let l=0;l<4;l++)A[j][l]+=z[i][j]*weights[i]*z[i][l]/n;}for(let j=0;j<4;j++)A[j][j]+=D.meta.ridge_lambda;hb=solve(A,b);change=Math.max(...hb.map((v,j)=>Math.abs(v-before[j])));hstates.push({k,before,beta:hb.slice(),change,A,b,rows:focus.map(i=>({res:res[i],weight:weights[i],pred:dot(z[i],hb)})),objective:mean(yc.map((v,i)=>M.huberLoss(v-dot(z[i],hb),delta)))+D.meta.ridge_lambda*sq(hb)/2});if(change<1e-6)break;}
 const q=quantile(z,dy,{focus}),corr=columns(z).map(a=>columns(z).map(b=>dot(a,b)/Math.sqrt(sq(a)*sq(b))));const vif=columns(z).map((c,j)=>{const other=z.map(r=>r.filter((_,k)=>k!==j)),sys=normalSystem(other,c,1e-8/n),fit=solve(sys.A,sys.b),sse=sum(c.map((v,i)=>(v-dot(other[i],fit))**2)),sst=sum(c.map(v=>(v-mean(c))**2)),r2=sst>1e-15?Math.max(0,1-sse/sst):0;return {r2,vif:r2<1-1e-15?1/(1-r2):1e6,sse,sst};});
 const magnitudes=columns(dx).map(c=>({mad:mad(c),p90:percentile(c,.9),p99:percentile(c,.99),max:Math.max(...c.map(Math.abs)),nonzero:c.filter(v=>v!==0).length}));
 const models={ridge:{beta:gaussEN.solution,intercept:ymean},elastic:{beta:en.beta,intercept:ymean},huber:{beta:hb,intercept:ymean},quantile:{beta:q.beta,intercept:q.intercept}};
 const rankings={};for(const [name,fit] of Object.entries(models)){const rows=fit.beta.map((beta,j)=>({j,beta,raw:stats[j].sd?beta/stats[j].sd:0,...Object.fromEntries(['mad','p90','p99','max'].map(k=>[k,stats[j].sd?Math.abs(beta/stats[j].sd)*magnitudes[j][k]:0])),reasons:[],ranks:{}}));const total=sum(rows.filter(r=>r.beta>0).map(r=>r.p90));rows.forEach(r=>r.share=r.beta>0&&total>1e-15?r.p90/total:0);for(const metric of ['p90','p99','max']){rows.filter(r=>r.beta>0&&r[metric]>0).sort((a,b)=>b[metric]-a[metric]||a.j-b.j).forEach((r,k)=>{r.ranks[metric]=k+1;if(k<2&&(name!=='quantile'||q.converged))r.reasons.push(metric);});}rankings[name]=rows;}
 const high=vif.map((v,j)=>({j,v:v.vif})).filter(r=>r.v>10),clusters=[];for(const r of high){const g=clusters.find(g=>Math.abs(corr[g[0]][r.j])>.8);if(g)g.push(r.j);else clusters.push([r.j]);}const groups=clusters.filter(g=>g.length>1).map(g=>{const c=dx.map(r=>sum(g.map(j=>r[j]))),cm=mean(c),varc=sum(c.map(v=>(v-cm)**2)),coef=varc?sum(c.map((v,i)=>(v-cm)*yc[i]))/varc:0;return {members:g,coef,impact:Math.abs(coef)*mad(c)};});
 return {version:1,n,xmean:stats.map(s=>s.mean),xstd:stats.map(s=>s.sd),ymean,ystd:ys.sd,gram:G.A,rhs:G.b,ridgeSystem,gauss:{en:gaussEN,pl:gaussPL},cv:crossValidation,elastic:en,huber:{delta,beta:hb,states:hstates},quantile:q,corr,vif,groups,magnitudes,models,rankings,topN:2};
}
module.exports={prepare,quantile,elastic,cv,cholesky,cholSolve,median,mad,percentile};
