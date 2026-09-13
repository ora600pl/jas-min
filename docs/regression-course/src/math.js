(function(root){
'use strict';
let language='en';
const text=(en,pl)=>language==='pl'?pl:en;
const setLanguage=value=>{language=value==='pl'?'pl':'en';};
const clone=a=>a.map(r=>r.slice());
const dot=(a,b)=>a.reduce((s,v,i)=>s+v*b[i],0);
const mean=a=>a.reduce((s,v)=>s+v,0)/a.length;
const norm=a=>Math.sqrt(dot(a,a));
// Fictional brewhouse fixtures shared by the narrative labs and their arithmetic tests.
// These are not process measurements or brewing instructions.
const brewing={
 reference:{volume:40,temperature:20,interruptions:0,minutes:120},
 pilot:[[40,20,0,120],[50,20,0,126],[40,25,0,110],[40,20,1,124]],
 next:{volume:50,temperature:18,interruptions:1,minutes:136},
 volume:[40,50,60],minutes:[120,126,129],
 ridgeX:[[-1,-1],[-1,1],[1,-1],[1,1]],
 elasticX:[[-1,-1],[-1,-.5],[1,.5],[1,1]],
 huberX:[-2,-1,0,1,2],huberY:[-12,-6,0,6,27]
};
brewing.beta=[(126-120)/(50-40),(110-120)/(25-20),(124-120)/(1-0)];
brewing.nextX=[10,-2,1];
brewing.X=brewing.volume.map(v=>[1,(v-40)/10]);
brewing.y=brewing.minutes.map(v=>v-120);
brewing.ridgeY=brewing.ridgeX.map(row=>dot(row,[6,-4]));
brewing.elasticY=brewing.elasticX.map(row=>dot(row,[6,-4]));
const brewLoss=b=>mean(brewing.y.map((v,i)=>(v-brewing.X[i][1]*b)**2))/2;
const brewGradient=b=>mean(brewing.X.map((row,i)=>row[1]*(row[1]*b-brewing.y[i])));
function parseNumber(input){
 const s=String(input).trim();
 if(!/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?(?:\s*\/\s*[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?)?$/i.test(s))throw Error(text('Enter a number (decimal point) or a fraction such as -3/2.','Wpisz liczbę (z kropką dziesiętną) lub ułamek, np. -3/2.'));
 const parts=s.split('/').map(Number);if(parts[1]===0)throw Error(text('Division by zero is not allowed.','Nie można dzielić przez zero.'));
 const v=parts.length===2?parts[0]/parts[1]:parts[0];if(!Number.isFinite(v)||Math.abs(v)>1e9)throw Error(text('Use a finite number with magnitude at most 1,000,000,000.','Wpisz skończoną liczbę o wartości bezwzględnej do 1 000 000 000.'));return v;
}
function standardize(a){if(a.length<2)throw Error(text('At least two values are required.','Potrzeba co najmniej dwóch wartości.'));const m=mean(a),variance=a.reduce((s,v)=>s+(v-m)**2,0)/(a.length-1),sd=Math.sqrt(variance);return {mean:m,variance,sd,z:a.map(v=>sd?(v-m)/sd:0),constant:sd===0};}
function normalSystem(X,y,lambda=0,penalizeIntercept=true){
 const n=X.length,p=X[0].length,A=Array.from({length:p},()=>Array(p).fill(0)),b=Array(p).fill(0);
 for(let i=0;i<n;i++)for(let j=0;j<p;j++){b[j]+=X[i][j]*y[i]/n;for(let k=0;k<p;k++)A[j][k]+=X[i][j]*X[i][k]/n;}
 for(let j=0;j<p;j++)if(penalizeIntercept||j>0)A[j][j]+=lambda;return {A,b};
}
function gaussian(A,b,pivoting=true){
 if(!A.length||A.some(r=>r.length!==A[0].length)||A.length!==b.length||!A.flat().concat(b).every(Number.isFinite))throw Error(text('Invalid system.','Nieprawidłowy układ.'));
 const n=A[0].length,M=A.map((r,i)=>[...r,b[i]]),scale=Math.max(1,...A.flat().map(Math.abs)),tol=1e-12*scale,steps=[];
 function save(title,explanation,extra={}){steps.push({matrix:clone(M),title,explanation,...extra});}
 save(text('Start with the augmented matrix','Zacznij od macierzy rozszerzonej'),text('The last column is the right-hand side. Every row operation must include it.','Ostatnia kolumna to prawa strona równań. Każda operacja na wierszu musi ją obejmować.'));
 let row=0;const pivots=[];
 for(let col=0;col<n&&row<M.length;col++){
  let best=row;if(pivoting)for(let i=row+1;i<M.length;i++)if(Math.abs(M[i][col])>Math.abs(M[best][col]))best=i;
  if(Math.abs(M[best][col])<=tol){save(text(`No pivot in column ${col+1}`,`Brak elementu głównego w kolumnie ${col+1}`),text('There is no usable coefficient in this column below the previous pivots. Continue to the next column.','W tej kolumnie nie ma użytecznego współczynnika poniżej poprzednich elementów głównych. Przejdź do kolejnej kolumny.'),{pivot:[row,col]});continue;}
  if(best!==row){[M[best],M[row]]=[M[row],M[best]];save(text(`Swap R${row+1} ↔ R${best+1}`,`Zamień R${row+1} ↔ R${best+1}`),text('Partial pivoting chooses the largest available absolute value in this column. Swapping whole equations preserves every solution.','Częściowy wybór elementu głównego wybiera największą dostępną wartość bezwzględną w kolumnie. Zamiana całych równań zachowuje wszystkie rozwiązania.'),{active:row,pivot:[row,col]});}
  save(text(`Pivot at row ${row+1}, column ${col+1}`,`Element główny: wiersz ${row+1}, kolumna ${col+1}`),text(`Use ${M[row][col]} as the divisor. We will cancel the entries below it, one row at a time.`,`Użyj ${M[row][col]} jako dzielnika. Wyzerujemy elementy poniżej, po jednym wierszu.`),{active:row,pivot:[row,col]});
  for(let i=row+1;i<M.length;i++){
   if(Math.abs(M[i][col])<=tol){M[i][col]=0;continue;}
   const numerator=M[i][col],denominator=M[row][col],factor=numerator/denominator,before=M[i].slice(),source=M[row].slice();
   for(let j=col;j<=n;j++)M[i][j]-=factor*M[row][j];M[i][col]=0;
   save(`R${i+1} ← R${i+1} − (${factor}) R${row+1}`,text(`Multiplier = ${numerator} / ${denominator}. Subtract that multiple of every entry, including the right-hand side.`,`Mnożnik = ${numerator} / ${denominator}. Odejmij tę wielokrotność każdego elementu, również prawej strony.`),{active:i,pivot:[row,col],factor,before,source});
  }
  pivots.push([row,col]);row++;
 }
 const inconsistent=M.some(r=>r.slice(0,n).every(v=>Math.abs(v)<=tol)&&Math.abs(r[n])>tol);
 if(inconsistent){save(text('No solution','Brak rozwiązania'),text('A row says 0 = a nonzero number. The original equations contradict one another.','Wiersz mówi: zero równa się liczbie niezerowej. Równania są sprzeczne.'));return {steps,status:'inconsistent',solution:null};}
 if(pivots.length<n){save(text('Infinitely many solutions','Nieskończenie wiele rozwiązań'),text('At least one variable has no pivot. A free variable remains; there is no unique solution.','Co najmniej jedna zmienna nie ma elementu głównego. Pozostaje zmienna swobodna; rozwiązanie nie jest jednoznaczne.'));return {steps,status:'underdetermined',solution:null};}
 save(text('Upper-triangular system','Układ górnotrójkątny'),text('Forward elimination is complete. Back-substitution now starts at the bottom.','Eliminacja zakończona. Podstawianie wsteczne zaczynamy od dołu.'));
 const x=Array(n).fill(0);
 for(let q=pivots.length-1;q>=0;q--){const [r,c]=pivots[q];let known=0;for(let j=c+1;j<n;j++)known+=M[r][j]*x[j];x[c]=(M[r][n]-known)/M[r][c];save(text(`Back-substitute x${c+1}`,`Podstaw wstecznie x${c+1}`),`x${c+1} = (${M[r][n]} − ${known}) / ${M[r][c]} = ${x[c]}. `+text('The known contribution comes from variables already solved.','Znany składnik pochodzi od już wyliczonych zmiennych.'),{active:r,pivot:[r,c],solution:x.slice(),solvedFrom:c});}
 const residual=A.map((r,i)=>dot(r,x)-b[i]);save(text('Check the original equations','Sprawdź oryginalne równania'),text(`Largest absolute substitution error: ${Math.max(...residual.map(Math.abs))}. Small floating-point roundoff is expected.`,`Największy bezwzględny błąd podstawienia: ${Math.max(...residual.map(Math.abs))}. Drobne błędy zaokrągleń są spodziewane.`),{solution:x.slice(),solvedFrom:0});
 return {steps,status:'unique',solution:x,residual};
}
function solve(A,b){const result=gaussian(A,b);if(!result.solution)throw Error(text('The system has no unique solution.','Układ nie ma jednoznacznego rozwiązania.'));return result.solution;}
function rowOperation(M,{kind,target,source,factor}){
 const n=M.length;if(!Number.isInteger(target)||target<0||target>=n)throw Error(text('Choose a valid target row.','Wybierz prawidłowy wiersz docelowy.'));
 if(kind==='scale'){if(!Number.isFinite(factor)||factor===0)throw Error(text('A row can only be multiplied by a nonzero finite number.','Wiersz można mnożyć tylko przez skończoną liczbę różną od zera.'));}
 else if(!Number.isInteger(source)||source<0||source>=n||source===target)throw Error(text('Choose two different rows.','Wybierz dwa różne wiersze.'));
 if(kind==='add'&&!Number.isFinite(factor))throw Error(text('Enter a finite multiplier.','Wpisz skończony mnożnik.'));
 const N=clone(M);if(kind==='swap')[N[target],N[source]]=[N[source],N[target]];
 else if(kind==='scale')N[target]=N[target].map(v=>v*factor);
 else if(kind==='add')N[target]=N[target].map((v,j)=>v+factor*N[source][j]);else throw Error(text('Unknown row operation.','Nieznana operacja na wierszu.'));
 if(!N.flat().every(v=>Number.isFinite(v)&&Math.abs(v)<1e15))throw Error(text('This operation is too large for the teaching calculator.','Wartości przekraczają limit kalkulatora edukacyjnego.'));return N;
}
function soft(c,t){return Math.sign(c)*Math.max(Math.abs(c)-t,0);}
function elasticTrace(X,y,lambda,alpha,maxPasses=20){
 const n=X.length,p=X[0].length,b=Array(p).fill(0),r=y.slice(),states=[{beta:b.slice(),iteration:0,label:text('All coefficients start at zero.','Wszystkie współczynniki zaczynają od zera.')}];
 for(let pass=1;pass<=maxPasses;pass++){const old=b.slice();for(let j=0;j<p;j++){
  const previous=b[j];for(let i=0;i<n;i++)r[i]+=X[i][j]*previous;
  const c=X.reduce((s,row,i)=>s+row[j]*r[i]/n,0),a=X.reduce((s,row)=>s+row[j]**2/n,0),threshold=lambda*alpha,denominator=a+lambda*(1-alpha);
  b[j]=soft(c,threshold)/denominator;for(let i=0;i<n;i++)r[i]-=X[i][j]*b[j];
  states.push({beta:b.slice(),iteration:pass,j,previous,c,a,threshold,denominator,residual:r.slice(),label:text(`Pass ${pass}, coefficient ${j+1}`,`Przebieg ${pass}, współczynnik ${j+1}`)});
 }if(Math.max(...b.map((v,j)=>Math.abs(v-old[j])))<1e-8)break;}
 return states;
}
function huberLoss(r,delta){return Math.abs(r)<=delta?r*r/2:delta*(Math.abs(r)-delta/2);}
function huberWeight(r,delta){return Math.abs(r)<=delta?1:delta/Math.abs(r);}
function huberTrace(X,y,delta,lambda=.05,max=40){
 const p=X[0].length,n=X.length;let beta=Array(p).fill(0);const states=[];
 for(let k=0;k<=max;k++){
  const residual=y.map((v,i)=>v-dot(X[i],beta)),weights=residual.map(r=>huberWeight(r,delta));states.push({iteration:k,beta:beta.slice(),residual,weights});
  const A=Array.from({length:p},()=>Array(p).fill(0)),b=Array(p).fill(0);
  for(let i=0;i<n;i++)for(let j=0;j<p;j++){b[j]+=X[i][j]*weights[i]*y[i]/n;for(let l=0;l<p;l++)A[j][l]+=X[i][j]*weights[i]*X[i][l]/n;}
  for(let j=0;j<p;j++)A[j][j]+=lambda;const next=solve(A,b);
  if(k>0&&Math.max(...beta.map((v,j)=>Math.abs(v-next[j])))<1e-8)break;beta=next;
 }return states;
}
function pinball(r,tau){return r>=0?tau*r:(tau-1)*r;}
function admmIntercept(y,tau=.75,rho=1,max=100){
 let a=0,r=y.slice(),u=y.map(()=>0);const states=[{iteration:0,a,r:r.slice(),u:u.slice(),primal:null,dual:null}];
 for(let k=1;k<=max;k++){
  const oldR=r.slice(),oldU=u.slice();a=mean(y.map((v,i)=>v-oldR[i]-oldU[i]));const v=y.map((value,i)=>value-a-oldU[i]);
  r=v.map(t=>t>tau/rho?t-tau/rho:t<(tau-1)/rho?t-(tau-1)/rho:0);
  const mismatch=y.map((value,i)=>a+r[i]-value);u=u.map((value,i)=>value+mismatch[i]);
  states.push({iteration:k,a,r:r.slice(),u:u.slice(),oldR,oldU,v,mismatch,primal:norm(mismatch),dual:rho*Math.abs(r.reduce((s,value,i)=>s+value-oldR[i],0)),objective:mean(y.map(value=>pinball(value-a,tau)))});
 }return states;
}
function parseCSV(source,asDifferences=false){
 if(source.length>2*1024*1024)throw Error(text('File limit: 2 MiB.','Limit pliku: 2 MiB.'));
 const lines=source.replace(/^\uFEFF/,'').trim().split(/\r?\n/);
 if(lines[0].trim()!=='y,x1,x2,x3,x4')throw Error(text('Required header: y,x1,x2,x3,x4 (comma delimiter).','Wymagany nagłówek: y,x1,x2,x3,x4 (separator: przecinek).'));
 if(lines.length<7||lines.length>50001)throw Error(text('Use 6–50,000 observations.','Użyj 6–50 000 obserwacji.'));
 let rows=lines.slice(1).map((line,i)=>{const values=line.split(',');if(values.length!==5)throw Error(text(`Row ${i+2}: expected five values.`,`Wiersz ${i+2}: oczekiwano pięciu wartości.`));return values.map(parseNumber);});
 if(asDifferences)rows=rows.slice(1).map((row,i)=>row.map((v,j)=>v-rows[i][j]));
 const y=rows.map(r=>r[0]),x=rows.map(r=>r.slice(1)),stats=[0,1,2,3].map(j=>standardize(x.map(r=>r[j]))),baseline=mean(y),z=x.map(row=>row.map((v,j)=>stats[j].sd?(v-stats[j].mean)/stats[j].sd:0));
 return {y,x,z,baseline,stats};
}
const api={setLanguage,brewing,brewLoss,brewGradient,clone,dot,mean,norm,parseNumber,standardize,normalSystem,gaussian,solve,rowOperation,soft,elasticTrace,huberLoss,huberWeight,huberTrace,pinball,admmIntercept,parseCSV};
if(typeof module!=='undefined'&&module.exports)module.exports=api;root.CourseMath=api;
})(typeof globalThis!=='undefined'?globalThis:this);
