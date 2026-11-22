import { defineConfig } from "vite";

// Three pages, each in its own webview: the browser chrome (tabs + toolbar), the
// spotlight palette overlay and the new-tab page shown inside content tabs.
export default defineConfig(() => ({
  clearScreen: false,
  build: {
    chunkSizeWarningLimit: 1500,
    rollupOptions: {
      input: { index: "index.html", palette: "palette.html", newtab: "newtab.html", settings: "settings.html", bookmarks: "bookmarks.html", omni: "omni.html", meet: "meet.html", cover: "cover.html", side: "side.html", siteinfo: "siteinfo.html", neterror: "neterror.html", home: "home.html", downloads: "downloads.html" },
    },
  },
  server: {
    port: 1440,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
}));
