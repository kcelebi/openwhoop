import { createRoot } from "react-dom/client";
import { lazy, Suspense } from "react";
import "./index.css";
const App = lazy(() => import("./App"));
const HostedDashboard = lazy(() => import("./HostedDashboard"));

// StrictMode is off so dev does not mount/unmount twice; that closes the first WebSocket and
// fills the log with spurious error/close events while the UI can still show "open".
createRoot(document.getElementById("root")!).render(<Suspense fallback={<div>Loading dashboard…</div>}>{import.meta.env.VITE_SUPABASE_URL ? <HostedDashboard /> : <App />}</Suspense>);
