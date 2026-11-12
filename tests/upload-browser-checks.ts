import type {Page} from '../web/node_modules/playwright';
import {mkdtemp,writeFile,rm,readFile,stat} from 'node:fs/promises';
import {join,resolve} from 'node:path';

export async function checkUploads(page:Page){
 const scratch=await mkdtemp('/tmp/rexweb-upload-browser.');const password='uploads-test-password-0123456789';
 await writeFile(join(scratch,'password'),password,{mode:0o600});
 const process=Bun.spawn([resolve('bin/rexweb'),'-mode','web','-listen','127.0.0.1:0','-uploads','-password-file',join(scratch,'password'),'-session-socket',join(scratch,'run/session.sock'),'-workspace',join(scratch,'work')],{stdout:'pipe',stderr:'pipe'});
 let logs='';void(async()=>{for await(const chunk of process.stderr)logs+=new TextDecoder().decode(chunk)})();
 const authorization='Basic '+btoa('rexweb:'+password);const assert=(value:unknown,message:string)=>{if(!value)throw new Error(message)};
 try{
  for(let i=0;i<100&&!logs.includes('Ready:');i++){assert(process.exitCode===null,'upload service exited: '+logs);await Bun.sleep(50)}
  const address=logs.match(/Web listener at (\S+)/)?.[1];assert(address,'upload service not ready');const origin='http://'+address;
  await page.setExtraHTTPHeaders({Authorization:authorization});const cdp=await page.context().newCDPSession(page);await cdp.send('Emulation.setTouchEmulationEnabled',{enabled:true,maxTouchPoints:1});
  const input:string[]=[];page.on('websocket',socket=>{if(!socket.url().startsWith(origin.replace('http:','ws:')))return;socket.on('framesent',frame=>{try{const[kind,data]=JSON.parse(String(frame.payload));if(kind==='stdin')input.push(data)}catch{}})});
  await page.goto(origin);await page.waitForSelector('.webterm-terminal.-connected');await page.locator('.webterm-terminal').click();
  const firstResponse=page.waitForResponse(response=>response.url().includes('/api/upload?')&&response.request().method()==='POST');
  await page.locator('[data-mobile-keyboard-input]').evaluate(element=>{
   const data=new DataTransfer();data.items.add(new File([new Uint8Array([137,80,78,71,13,10,26,10])],'../../image.png',{type:'image/png'}));
   element.dispatchEvent(new ClipboardEvent('paste',{clipboardData:data,bubbles:true,cancelable:true}));
  });
  const first=await firstResponse;assert(first.ok(),'image upload failed');const result=await first.json();
  assert(result.name.startsWith('upload-')&&result.name.endsWith('.png'),'unsafe upload name');assert(result.path.startsWith(join(scratch,'work')+'/'),'upload escaped workspace');
  assert((await readFile(result.path)).length===8&&(await stat(result.path)).mode%0o1000===0o600,'upload bytes or permissions wrong');
  await page.waitForTimeout(100);assert(input.join('').includes(result.path)&&!input.join('').includes('\r')&&!input.join('').includes('\n'),'upload path was not safely typed without Enter');
  const body=()=>{const data=new FormData();data.append('file',new File(['blocked'],'file.txt'));return data};
  const query=new URL(first.url()).search;
  const anonymous=await fetch(origin+'/api/upload'+query,{method:'POST',headers:{Origin:origin},body:body()});assert(anonymous.status===401,'anonymous upload accepted');
  const cross=await fetch(origin+'/api/upload'+query,{method:'POST',headers:{Origin:'https://evil.example',Authorization:authorization},body:body()});assert(cross.status===403,'cross-origin upload accepted');
  const secondResponse=page.waitForResponse(response=>response.url().includes('/api/upload?')&&response.request().method()==='POST');
  await page.locator('#upload-file').setInputFiles({name:'notes.txt',mimeType:'text/plain',buffer:Buffer.from('second upload')});
  const second=await secondResponse;assert(second.ok(),'file picker upload failed');const secondResult=await second.json();assert((await readFile(secondResult.path)).toString()==='second upload','file upload changed bytes');
  console.log('Upload browser checks passed: clipboard image, file picker, confined names/permissions, auth/origin and path insertion without Enter.');
 }finally{await page.setExtraHTTPHeaders({});process.kill();await process.exited;await rm(scratch,{recursive:true,force:true})}
}
