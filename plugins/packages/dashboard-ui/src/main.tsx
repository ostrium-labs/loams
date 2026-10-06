import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { CoreShell } from "@loams-plugins/core/ui";
import App from "./App";
import "./styles.css";

/*
 * Plugin state is server state, so TanStack Query owns it for the whole app.
 * The provider sits above `CoreShell` (which supplies the router) so the
 * console, the plugins page and the plugin page all share one cache entry.
 *
 * There are no react-router loaders anywhere in this app: a loader would
 * populate a second cache for the same `['plugins']` key and the two would
 * disagree after the first toggle.
 */
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // A failed read is worth retrying -- the API is usually just not up yet
      // during development -- but not forever.
      retry: 2,
      staleTime: 5_000,
      refetchOnWindowFocus: false,
    },
  },
});

/*
 * The shell owns routing and the plugin chrome; `App` is handed to it as the
 * dashboard plugin's own surface so `/` and `/plugins/dashboard` render the
 * same page instead of the shell importing dashboard-ui from the other side.
 */
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <CoreShell dashboard={App} />
    </QueryClientProvider>
  </React.StrictMode>,
);
