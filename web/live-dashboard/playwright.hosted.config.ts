import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
 testDir:'./e2e',testMatch:'hosted-desktop.spec.ts',workers:1,reporter:'list',
 use:{baseURL:'http://127.0.0.1:5217',...devices['Desktop Chrome']},
 webServer:{command:'npm run dev -- --host 127.0.0.1 --port 5217 --strictPort',
  env:{VITE_SUPABASE_URL:'https://test.invalid',VITE_SUPABASE_PUBLISHABLE_KEY:'test-publishable'},
  url:'http://127.0.0.1:5217',reuseExistingServer:!process.env.CI,timeout:30000}
});
