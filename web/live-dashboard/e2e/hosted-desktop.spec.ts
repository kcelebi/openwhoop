import { test, expect, devices, type Page } from "@playwright/test";
const owner='00000000-0000-4000-8000-000000000001';
const fixed=Date.UTC(2026,9,31,18,0);
const token=Buffer.from(JSON.stringify({alg:'HS256',typ:'JWT'})).toString('base64url')+'.'+Buffer.from(JSON.stringify({sub:owner,exp:2000000000,role:'authenticated'})).toString('base64url')+'.test';
async function prepare(page: Page){
 await page.clock.install({time:new Date(fixed)});
 await page.addInitScript(({owner,token})=>{const value=JSON.stringify({access_token:token,refresh_token:'test-refresh',expires_at:2000000000,expires_in:1000000,token_type:'bearer',user:{id:owner,aud:'authenticated',role:'authenticated',email:'test@example.invalid',app_metadata:{},user_metadata:{},created_at:'2026-01-01T00:00:00Z'}});for(const key of ['sb-test-auth-token','sb-kqqnicyuottynkipoevc-auth-token'])localStorage.setItem(key,value)}, {owner,token});
 let calls=0;
 await page.route(/https:\/\/(test\.invalid|kqqnicyuottynkipoevc\.supabase\.co)\//,async route=>{
  const u=new URL(route.request().url());let data=[];
  if(u.pathname.endsWith('whoop_hr_samples')){calls++;data=Array.from({length:180},(_,i)=>({recorded_at_ms:fixed+calls*3000-i*1000,bpm:65+i%12}));}
  else if(u.pathname.endsWith('whoop_relay_status'))data={last_seen_at:new Date(fixed+calls*3000).toISOString(),connected:true,battery_percent:90,battery_updated_at:new Date(fixed).toISOString(),strap_alarm_enabled:true,strap_alarm_unix:Math.floor(fixed/1000)+3600};
  else if(u.pathname.endsWith('whoop_alarms'))data=[{id:'test-alarm',label:'Alarm',kind:'cron',cron_expr:'0 0 15 * * *',timezone:'America/New_York',one_time_unix:null,next_unix:Math.floor(fixed/1000)+3600,enabled:true}];
  else if(u.pathname.endsWith('whoop_insights'))data=[{period_key:'2026-10-31',timezone:'America/New_York',calculated_at:new Date(fixed).toISOString(),result:{sleep:{status:'collecting',episodes:[],duration_minutes:0,coverage:0,motion_seconds:0,note:'Estimated from recorded motion.'},activity:{episodes:[],duration_minutes:0,threshold_bpm:120,note:'Elevated pulse estimate.'},pulse:{coverage:0.25,rr_intervals:0,rmssd_ms:null}}}];
  else if(u.pathname.includes('whoop_pulse_history')){const body=route.request().postDataJSON();const step=(body.end_ms-body.start_ms)/60;data={step_ms:step,points:Array.from({length:60},(_,i)=>({t:body.start_ms+i*step,bpm:65+i%15,low:60+i%8,high:80+i%9,samples:step/1000})),summary:{average:72,low:60,high:90,coverage:.75,samples:3600,observed_seconds:3600},available_from_ms:fixed-604800000};}
  await route.fulfill({status:200,contentType:'application/json',body:JSON.stringify(data)});
 });
}

test('desktop history navigation and Alarms isolation',async({page})=>{
 await prepare(page);await page.goto('/');
 await expect(page.getByRole('heading',{name:'Pulse history',exact:true})).toBeVisible();
 await expect(page.getByText('Average',{exact:false}).first()).toBeVisible();
 await page.getByRole('button',{name:'7d',exact:true}).click();
 await expect(page.getByRole('button',{name:'7d',exact:true})).toHaveAttribute('aria-pressed','true');
 await page.getByRole('button',{name:'Previous period'}).click();
 await page.getByRole('button',{name:'Custom',exact:true}).click();
 await page.getByLabel('From',{exact:true}).fill('2026-04-27T00:00');
 await page.getByLabel('To',{exact:true}).fill('2026-04-28T00:00');
 await expect(page.getByText('Average',{exact:false}).first()).toBeVisible();
 await page.getByRole('button',{name:'Back to live'}).click();
 await expect(page.getByRole('button',{name:'1h',exact:true})).toHaveAttribute('aria-pressed','true');
 await page.getByRole('button',{name:'Alarms',exact:true}).click();
 await expect(page.getByRole('heading',{name:'Pulse history',exact:true})).toHaveCount(0);
});
test('iPhone portrait and landscape preserve mobile Pulse and daily alarms',async({browser,baseURL})=>{
 const context=await browser.newContext({...devices['iPhone 15']});const page=await context.newPage();let requests=0;
 page.on('request',r=>{if(r.url().includes('whoop_pulse_history'))requests++});
 await prepare(page);await page.goto(baseURL!);
 await expect(page.getByRole('heading',{name:'Recent pulse',exact:true})).toBeVisible();
 await expect(page.getByRole('heading',{name:'Pulse history',exact:true})).toHaveCount(0);
 await page.setViewportSize({width:852,height:393});
 await expect(page.getByRole('heading',{name:'Recent pulse',exact:true})).toBeVisible();
 await expect(page.getByRole('heading',{name:'Pulse history',exact:true})).toHaveCount(0);
 await page.setViewportSize({width:393,height:852});
 await page.getByRole('button',{name:'Alarms',exact:true}).click();await page.getByRole('button',{name:'+ Add',exact:true}).click();
 await expect(page.getByLabel('Hour',{exact:true})).toHaveValue('7');
 await expect(page.getByLabel('Minute',{exact:true})).toHaveValue('00');
 await expect(page.locator('.repeat-settings')).not.toHaveAttribute('open','');
 await expect(page.locator('.repeat-settings summary')).toContainText('Every day');
 expect(await page.evaluate(()=>document.body.scrollWidth<=innerWidth)).toBe(true);expect(requests).toBe(0);await context.close();
});
