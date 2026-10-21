// Run against an isolated emulator serving the real cores3_studio.html.
const fs=require('node:fs');
const assert=require('node:assert/strict');
const net=require('node:net');
const {chromium}=require(process.env.PLAYWRIGHT_CORE_PATH||'/data/data/com.termux/files/usr/lib/node_modules/@playwright/cli/node_modules/playwright-core');
const [url,out]=process.argv.slice(2);
if(!url||!out)throw Error('usage: node cores3-camera-browser.cjs URL OUTPUT_DIRECTORY');
fs.mkdirSync(out,{recursive:true});
(async()=>{
 const sockets=new Set();let proxy;
 const report={errors:[],sent:0,preview:0,stats:[],fragmentedMessages:0};
 let socketPort=new URL(url).port;
 if(process.argv.includes('--fragment-camera')){
  proxy=net.createServer(client=>{
   const upstream=net.connect(Number(new URL(url).port),'127.0.0.1');
   for(const socket of [client,upstream]){sockets.add(socket);socket.on('close',()=>sockets.delete(socket));socket.on('error',()=>{client.destroy();upstream.destroy();});}
   upstream.pipe(client);let pending=Buffer.alloc(0),headers=true;
   client.on('data',chunk=>{
    pending=Buffer.concat([pending,chunk]);
    if(headers){const end=pending.indexOf('\r\n\r\n');if(end<0)return;headers=false;
     // the emulator only accepts pages on its own listener: the proxy speaks with its own Origin
     let head=Buffer.from(pending.subarray(0,end+4).toString('latin1').replace(/Origin: http:\/\/(127\.0\.0\.1|localhost):\d+/i,`Origin: http://127.0.0.1:${socketPort}`),'latin1');
     upstream.write(head);pending=pending.subarray(end+4);}
    while(pending.length>=2){
     const opcode=pending[0]&15,masked=!!(pending[1]&128);let length=pending[1]&127,header=2;
     if(length===126){if(pending.length<4)return;length=pending.readUInt16BE(2);header=4;}
     else if(length===127){if(pending.length<10)return;length=Number(pending.readBigUInt64BE(2));header=10;}
     const body=header+(masked?4:0),total=body+length;if(pending.length<total)return;
     const packet=pending.subarray(0,total);pending=pending.subarray(total);
     if(opcode!==2||length!==76805){upstream.write(packet);continue;}
     const data=Buffer.from(packet.subarray(body));if(masked)for(let i=0;i<data.length;i++)data[i]^=packet[header+(i&3)];
     if(data[0]!==3){upstream.write(packet);continue;}
     report.fragmentedMessages++;
     for(let offset=0;offset<data.length;offset+=32768){
      const chunk=data.subarray(offset,offset+32768),h=Buffer.alloc(8);
      h[0]=(offset+chunk.length===data.length?128:0)|(offset===0?2:0);h[1]=128|126;h.writeUInt16BE(chunk.length,2);
      upstream.write(Buffer.concat([h,chunk]));
     }
    }
   });
   client.on('end',()=>upstream.end());
  });
  await new Promise(resolve=>proxy.listen(0,'127.0.0.1',resolve));socketPort=String(proxy.address().port);
 }
 const browser=await chromium.launch({executablePath:process.env.CHROMIUM_PATH||'/data/data/com.termux/files/usr/bin/chromium-browser',headless:true,args:['--no-sandbox','--disable-dev-shm-usage','--use-fake-ui-for-media-stream','--use-fake-device-for-media-stream',`--use-file-for-fake-video-capture=${out}/camera.y4m`]});
 let page;
 try{
  page=await browser.newPage({viewport:{width:1100,height:1100}});
  page.on('pageerror',e=>report.errors.push(e.message));
  page.on('websocket',ws=>{
   ws.on('framesent',e=>{if(Buffer.isBuffer(e.payload)&&e.payload[0]===3)report.sent++;});
   ws.on('framereceived',e=>{
    if(Buffer.isBuffer(e.payload)&&e.payload[0]===4)report.preview++;
    else if(typeof e.payload==='string'){try{const d=JSON.parse(e.payload);if(d.t==='stat'){report.stats.push(d);if(report.stats.length>20)report.stats.shift();}}catch{}}
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
  await tap(120,110);
  await page.waitForFunction(()=>window.testStats?.cam>0,null,{timeout:25000});
  await page.locator('#cores3Canvas').screenshot({path:`${out}/guest-before.png`});
  await page.click('#btnToggleCam');
  await page.waitForFunction(()=>document.querySelector('#camStatusBadge').textContent.startsWith('送信中'));
  // These pixels return from the Rust board, rather than the local video preview.
  const pixel=()=>page.evaluate(()=>Array.from(document.querySelector('#backendCamPreview').getContext('2d').getImageData(160,120,1,1).data));
  await page.waitForFunction(()=>{
   const p=document.querySelector('#backendCamPreview').getContext('2d').getImageData(160,120,1,1).data;
   return p[0]>200&&p[1]<30&&p[2]<30;
  },null,{timeout:12000});
  report.red=await pixel();
  await page.locator('#backendCamPreview').screenshot({path:`${out}/red-return.png`});
  await page.waitForFunction(()=>{
   const p=document.querySelector('#cores3Canvas').getContext('2d').getImageData(100,140,1,1).data;
   return p[0]>200&&p[1]<40&&p[2]<40;
  },null,{timeout:12000});
  await page.locator('#cores3Canvas').screenshot({path:`${out}/guest-red.png`});

  await page.waitForFunction(()=>{
   const p=document.querySelector('#backendCamPreview').getContext('2d').getImageData(160,120,1,1).data;
   return p[0]<30&&p[1]>200&&p[2]<30;
  },null,{timeout:12000});
  report.green=await pixel();
  await page.locator('#backendCamPreview').screenshot({path:`${out}/green-return.png`});
  await page.waitForFunction(()=>{
   const p=document.querySelector('#cores3Canvas').getContext('2d').getImageData(100,140,1,1).data;
   return p[0]<40&&p[1]>200&&p[2]<40;
  },null,{timeout:12000});
  await page.locator('#cores3Canvas').screenshot({path:`${out}/guest-green.png`});
  await page.click('#btnCamStop');
  assert(report.sent>=2&&report.preview>=2);
  assert.deepEqual(report.errors,[]);
  report.success=true;
 }finally{
  if(page){report.status=await page.locator('#camStatusBadge').textContent();report.video=await page.evaluate(()=>{const v=document.querySelector('#webcamVideo');return v&&{width:v.videoWidth,height:v.videoHeight,paused:v.paused};});await page.screenshot({path:`${out}/page.png`,fullPage:true,timeout:5000}).catch(()=>{});}
  fs.writeFileSync(`${out}/result.json`,JSON.stringify(report,null,2));
  await browser.close();
  for(const socket of sockets)socket.destroy();
  if(proxy)await new Promise(resolve=>proxy.close(resolve));
 }
 console.log('PASS: real camera stream -> browser WebSocket -> Rust board -> red and green returned frames');
})().catch(e=>{console.error(e);process.exitCode=1;});
