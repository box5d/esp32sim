// Run against an isolated emulator serving the real cores3_studio.html.
const fs=require('node:fs');
const assert=require('node:assert/strict');
const {chromium}=require(process.env.PLAYWRIGHT_CORE_PATH||'/data/data/com.termux/files/usr/lib/node_modules/@playwright/cli/node_modules/playwright-core');
const [url,out]=process.argv.slice(2);
if(!url||!out)throw Error('usage: node cores3-camera-browser.cjs URL OUTPUT_DIRECTORY');
fs.mkdirSync(out,{recursive:true});
(async()=>{
 const report={errors:[],stats:[],sent:0};
 const socketPort=new URL(url).port;
 const browser=await chromium.launch({executablePath:process.env.CHROMIUM_PATH||'/data/data/com.termux/files/usr/bin/chromium-browser',headless:true,args:['--no-sandbox','--disable-dev-shm-usage']});
 let page;
 try{
  page=await browser.newPage({viewport:{width:1100,height:1100}});
  page.on('pageerror',e=>report.errors.push(e.message));
  page.on('websocket',ws=>{
   ws.on('framesent',e=>{if(typeof e.payload==='string'&&JSON.parse(e.payload).t==='imu')report.sent++;});
   ws.on('framereceived',e=>{
    if(typeof e.payload==='string'){try{const d=JSON.parse(e.payload);if(d.t==='stat'){report.stats.push(d);if(report.stats.length>20)report.stats.shift();}}catch{}}
   });
  });
  await page.addInitScript(({port})=>{
   const Native=window.WebSocket;
   window.WebSocket=class extends Native{constructor(url,...args){
    super(String(url).replace(':8088/ws',`:${port}/ws`),...args);
    this.addEventListener('message',e=>{if(typeof e.data==='string'){try{const d=JSON.parse(e.data);if(d.t==='stat')window.testStats=d;}catch{}}});
   }};
  },{port:socketPort});
  await page.route('**/api/action',async route=>{
   const data=route.request().postDataJSON();
   assert(['cores3_status','cores3_list_bins'].includes(data.action),'No lifecycle mutations in browser proof');
   await route.fulfill({contentType:'application/json',body:JSON.stringify({status:'success',running:true,bins:[]})});
  });
  await page.goto(url);
  await page.waitForFunction(()=>document.querySelector('#statusText').textContent==='実行中');
  await page.waitForFunction(()=>window.testStats?.time>=6,null,{timeout:30000});
  const tap=async(x,y)=>{
   const box=await page.locator('#cores3Canvas').boundingBox();
   await page.mouse.move(box.x+x/320*box.width,box.y+y/240*box.height);
   await page.mouse.down();await page.waitForTimeout(350);await page.mouse.up();
  };
  await tap(160,120);
  await page.waitForFunction(()=>window.testStats?.time>=8,null,{timeout:20000});
  await tap(40,180);
  await page.waitForTimeout(1500);

  await page.click('.tab-btn[data-tab="sensors"]');
  await page.evaluate(()=>{
   window.DeviceMotionEvent=class {static requestPermission(){return Promise.resolve('granted');}};
   window.pose=(x,y,z)=>{
    const event=new Event('devicemotion');
    Object.defineProperties(event,{accelerationIncludingGravity:{value:{x,y,z}},rotationRate:{value:{alpha:0,beta:0,gamma:0}}});
    window.dispatchEvent(event);
   };
  });
  await page.click('#btnImuStart');
  for(const [label,pose] of [['flat',[0,0,9.80665]],['tilted',[6.934,0,6.934]],['flat-return',[0,0,9.80665]]]){
   await page.evaluate(p=>{clearInterval(window.poseTimer);window.poseTimer=setInterval(()=>window.pose(...p),60);},pose);
   await page.waitForTimeout(7000);
   await page.locator('#cores3Canvas').screenshot({path:`${out}/${label}.png`});
   report[label]=await page.evaluate(()=>Array.from(document.querySelector('#cores3Canvas').getContext('2d').getImageData(90,80,140,130).data));
  }
  const different=(a,b)=>a.reduce((n,v,i)=>n+(Math.abs(v-b[i])>30),0);
  report.changed=different(report.flat,report.tilted);
  report.returnDifference=different(report.flat,report['flat-return']);
  for(const label of ['flat','tilted','flat-return']) delete report[label];
  assert(report.sent>20);
  assert(report.changed>500,'tilt must change guest LCD');
  assert(report.returnDifference<report.changed,'returning flat must restore guest LCD');
  await page.click('#btnImuStop');
  assert.deepEqual(report.errors,[]);
  report.success=true;
 }finally{
  if(page){report.status=await page.locator('#imuStatusBadge').textContent();await page.screenshot({path:`${out}/page.png`,fullPage:true,timeout:5000}).catch(()=>{});}
  fs.writeFileSync(`${out}/result.json`,JSON.stringify(report,null,2));
  await browser.close();
 }
 console.log('PASS: motion events -> browser WebSocket -> BMI270 -> official demo LCD, tilt and return');
})().catch(e=>{console.error(e);process.exitCode=1;});
