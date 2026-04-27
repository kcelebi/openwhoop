import { createRoot } from "react-dom/client";
import "./index.css";
import App from "./App";

// StrictMode is off so dev does not mount/unmount twice; that closes the first WebSocket and
// fills the log with spurious error/close events while the UI can still show "open".
createRoot(document.getElementById("root")!).render(<App />);
