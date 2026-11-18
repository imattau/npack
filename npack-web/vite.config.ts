import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  build: {
    assetsDir: 'assets',
    rollupOptions: {
      output: { entryFileNames: 'assets/app.js', chunkFileNames: 'assets/[name].js', assetFileNames: 'assets/app.[ext]' },
    },
  },
});
