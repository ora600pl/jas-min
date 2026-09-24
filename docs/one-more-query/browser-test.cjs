'use strict';
const assert=require('node:assert/strict'),{chromium}=require('playwright');
const fs=require('node:fs'),path=require('node:path'),os=require('node:os');
const artifacts=fs.mkdtempSync(path.join(os.tmpdir(),'one-more-query-browser-'));
let browser;
(async()=>{
 browser=await chromium.launch({headless:true,executablePath:process.env.BROWSER_PATH||chromium.executablePath()});
 const page=await browser.newPage({viewport:{width:1440,height:1050},reducedMotion:'reduce'}),errors=[],requests=[];
 page.on('pageerror',e=>errors.push(e.message));page.on('request',r=>requests.push(r.url()));
 const url=process.env.PREVIEW_URL||'http://127.0.0.1:8768/';let checks=0;
 const check=(condition,label)=>{assert.ok(condition,label);checks++;};
 for(const lang of ['pl','en'])for(const width of [320,390,900,1440]){
  await page.setViewportSize({width,height:1050});
  for(let scene=0;scene<5;scene++){
   await page.goto(url+'#'+lang+'/'+scene);await page.waitForFunction(()=>!!window.OneMoreQuery);
   check(await page.locator('h1').isVisible(),`${lang} ${width} scene ${scene} heading`);
   const overflow=await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1);
   check(!overflow,`${lang} ${width} scene ${scene} overflow`);
  }
 }
 await page.setViewportSize({width:1440,height:1050});await page.goto(url+'#pl/0');
 await page.locator('#separate').click();check(await page.locator('#separate').getAttribute('aria-pressed')==='true','separate scales');
 await page.locator('[data-guess="1"]').click();check((await page.locator('#guess-feedback').innerText()).includes('cursor: pin S wait on X'),'recorded prediction');check(await page.locator('[data-guess="1"]').evaluate(e=>document.activeElement===e),'prediction retains keyboard focus');
 await page.screenshot({path:path.join(artifacts,'noise.png'),fullPage:true});
 await page.locator('#language').click();check((await page.locator('#guess-feedback').innerText()).includes('Your pick'),'English preserves prediction');
 await page.locator('[data-scene="1"]').click();await page.locator('#volume').fill('1000');await page.locator('#volume').dispatchEvent('input');check((await page.locator('#token-estimate').innerText()).includes('250'),'context slider');
 await page.locator('#local-route').click();check((await page.locator('#route-feedback').innerText()).includes('entry points'),'local-route feedback');check(await page.locator('#local-route').getAttribute('aria-pressed')==='true','local route visibly selected');
 await page.locator('[data-scene="2"]').click();
 for(let k=0;k<6;k++){await page.locator('[data-stage="'+k+'"]').click();check((await page.locator('#calc-content').innerText()).length>100,'calculation stage '+k);}
 await page.screenshot({path:path.join(artifacts,'distillery.png'),fullPage:true});
 await page.locator('[data-stage="3"]').click();
 for(const lang of ['pl','en']){
  if(await page.locator('html').getAttribute('lang')!==lang)await page.locator('#language').click();
  for(const model of ['ridge','elastic','huber','quantile']){
   const trigger=page.locator('[data-explain-model="'+model+'"]');await trigger.click();
   check(await page.locator('#model-dialog').isVisible(),lang+' '+model+' beer dialog opens');
   check((await page.locator('#beer-body').innerText()).length>250,lang+' '+model+' plain explanation');
   check(await page.locator('.beer-maths').getAttribute('open')===null,lang+' '+model+' maths starts hidden');
   const savedFit=JSON.stringify(await page.evaluate(()=>window.OneMoreQuery.getFit().beta));
   const firstIntuition=await page.locator('#intuition-result').innerText();await page.locator('#intuition-slider').press('End');check((await page.locator('#intuition-result').innerText())!==firstIntuition,lang+' '+model+' plain experiment updates before opening maths');
   if(model==='ridge'){await page.locator('#intuition-slider').fill('1.1');await page.locator('#intuition-slider').dispatchEvent('input');check((await page.locator('#intuition-result').innerText()).includes(lang==='pl'?'1,05':'1.05'),lang+' Ridge sensitivity numbers');}
   if(model==='elastic'){await page.locator('#intuition-slider').fill('0.6');await page.locator('#intuition-slider').dispatchEvent('input');const zero=await page.locator('#intuition-result').innerText();await page.locator('#intuition-sparsity').uncheck();check((await page.locator('#intuition-result').innerText())!==zero,lang+' removing L1 removes exact zero');await page.locator('#intuition-sparsity').check();await page.locator('#intuition-slider').fill('-0.5');await page.locator('#intuition-slider').dispatchEvent('input');check((await page.locator('#intuition-result').innerText()).includes(lang==='pl'?'nowy mnożnik: 0':'new multiplier: 0'),lang+' negative input yields readable positive zero');}
   if(model==='huber'){await page.locator('#intuition-slider').fill('30');await page.locator('#intuition-slider').dispatchEvent('input');check((await page.locator('#intuition-result').innerText()).includes(lang==='pl'?'10,75':'10.75'),lang+' Huber robust centre');}
   if(model==='quantile'){await page.locator('#intuition-frequency').selectOption('20');await page.locator('#intuition-slider').fill('10');await page.locator('#intuition-slider').dispatchEvent('input');check((await page.locator('#intuition-result').innerText()).includes('20/21'),lang+' Q95 is not the maximum');}
   check(savedFit===JSON.stringify(await page.evaluate(()=>window.OneMoreQuery.getFit().beta)),lang+' '+model+' toy does not change actual fit');
   for(const width of [320,390,1440]){await page.setViewportSize({width,height:950});await page.locator('#intuition-slider').scrollIntoViewIfNeeded();check(await page.locator('#beer-body').evaluate(e=>e.scrollWidth<=e.clientWidth+1),lang+' '+model+' dialog fits '+width);if(width!==390)await page.screenshot({path:path.join(artifacts,lang+'-'+model+'-'+width+'.png')});}
   await page.locator('.beer-maths summary').click();check(await page.locator('.beer-maths').getAttribute('open')!==null,lang+' '+model+' maths expands');
   const originalLab=await page.locator('#model-lab-result').innerText();await page.locator('#model-lab-slider').press('End');check((await page.locator('#model-lab-result').innerText())!==originalLab,lang+' '+model+' worked lab updates');
   if(model==='quantile'){
    for(const count of ['4','19','20']){await page.locator('#q-frequency').selectOption(count);for(const prediction of ['10','30']){await page.locator('[data-lab-value="'+prediction+'"]').click();const expected=prediction==='10'?19:count==='4'?4:count==='19'?19:20;check((await page.locator('.lab-readout').innerText()).includes(String(expected)),lang+' Q95 '+count+' '+prediction);}await page.locator('[data-q-mean]').click();check(Math.abs(Number(await page.locator('#model-lab-slider').inputValue())-(10*Number(count)+30)/(Number(count)+1))<1e-9,lang+' Q95 exact mean at frequency '+count);}
   }
   await page.keyboard.press('Escape');check(await trigger.evaluate(e=>document.activeElement===e),lang+' '+model+' keyboard focus restored');
  }
 }
 await page.locator('[data-scene="3"]').click();check((await page.locator('.rank-name').first().innerText()).includes('cursor'),'P99 first');
 await page.locator('#percentile-details summary').click();check(await page.locator('.percentile-queue i').count()===101,'101-item percentile illustration');
 check((await page.locator('.provenance').innerText()).includes('19,369.42'),'real P99 interpolation shown');
 await page.locator('#show-beta-system').click();check(await page.locator('#ridge-details').getAttribute('open')!==null,'beta provenance opens actual system');await page.locator('#ridge-details > summary').click();
 await page.locator('#metric-select').selectOption('p90');check((await page.locator('.rank-name').first().innerText()).includes('PX'),'P90 first');
 const before=await page.evaluate(()=>window.OneMoreQuery.getFit().beta);await page.locator('#lambda').fill('0');await page.locator('#lambda').dispatchEvent('input');const after=await page.evaluate(()=>window.OneMoreQuery.getFit().beta);check(JSON.stringify(before)!==JSON.stringify(after),'live local Ridge');
 await page.locator('#ridge-details > summary').click();await page.locator('#gauss-step').fill('6');await page.locator('#gauss-step').dispatchEvent('input');check((await page.locator('#gauss-state').innerText()).includes('R4'),'Gaussian elimination controls');await page.locator('#gauss-step').press('End');check((await page.locator('#gauss-state').innerText()).includes('Back substitution: β1'),'final back substitution');
 await page.locator('#model-select').selectOption('quantile');check((await page.locator('#rank-content').innerText()).includes('excluded from classification'),'Q95 gate');
 await page.locator('[data-scene="4"]').click();await page.locator('.packet-preview summary').click();const preview=JSON.parse(await page.locator('#packet-preview').innerText());check(preview.method.ridgeLambda===1,'packet preview uses current lambda');check(preview.nextRequest.arguments.event_name===preview.entryPoints[0].event,'proposed request follows leader');check((await page.locator('.receipt-settings').innerText()).includes('λ = 1'),'receipt carries current lambda');
 for(const decision of ['tune','all','evidence']){await page.locator('[data-decision="'+decision+'"]').click();check((await page.locator('#decision-feedback').innerText()).length>90,'decision '+decision);}
 check((await page.locator('#decision-feedback').innerText()).includes('NOT EXECUTED'),'no fake MCP execution');
 for(const quiz of ['believe','drop','verify']){await page.locator('[data-quiz="'+quiz+'"]').click();check((await page.locator('#quiz-feedback').innerText()).length>80,'quality quiz '+quiz);}
 const dl=page.waitForEvent('download');await page.locator('#download-packet').click();const downloaded=await dl;const payload=JSON.parse(fs.readFileSync(await downloaded.path(),'utf8'));check(payload.models.quantile.eligible===false,'downloaded packet excludes Q95');check(payload.quality.missingSourceEntries[3].zeroFilled===963,'download retains quality caveat');
 await page.locator('#about').click();check(await page.locator('#data-dialog').isVisible(),'data modal');await page.keyboard.press('Escape');
 await page.setViewportSize({width:390,height:844});await page.goto(url+'#pl/2');await page.screenshot({path:path.join(artifacts,'mobile.png'),fullPage:true});
 // Hash navigation intentionally preserves choices; reload starts a fresh motion test.
 await page.emulateMedia({reducedMotion:'no-preference'});await page.goto(url+'#en/0');await page.reload();await page.waitForSelector('#chart-2 path');
 const firstPath=await page.locator('#chart-2 path').getAttribute('d');await page.locator('#separate').click();await page.waitForFunction(before=>document.querySelector('#chart-2 path').getAttribute('d')!==before,firstPath);check(true,'animated scale transition changes plotted geometry');
 await page.locator('[data-scene="2"]').click();await page.locator('#play-flow').click();check(await page.locator('#apparatus').evaluate(e=>e.classList.contains('running')),'data-flow animation starts');await page.locator('#motion').click();check(await page.locator('body').evaluate(e=>e.classList.contains('reduce-motion')),'motion can be disabled');
 check(errors.length===0,'no browser exceptions: '+errors.join('; '));check(requests.every(u=>u.startsWith(url)||u.startsWith('blob:')),'no external requests');
 const filePage=await browser.newPage();await filePage.goto('file://'+path.join(__dirname,'index.html')+'#en/3');await filePage.waitForFunction(()=>!!window.OneMoreQuery);check((await filePage.locator('.rank-name').first().innerText()).includes('cursor'),'direct file:// launch');
 await browser.close();console.log(JSON.stringify({checks,errors,networkOrigins:[...new Set(requests.map(r=>new URL(r).origin))],directFile:true,artifacts},null,2));
})().catch(async e=>{if(browser)await browser.close();console.error(e);process.exitCode=1;});
