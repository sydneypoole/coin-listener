import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import semiTheming from '@douyinfe/semi-vite-plugin';

export default defineConfig({
  plugins: [
    tailwindcss(),
    semiTheming({ cssLayer: true }),
    react(),
  ],
  server: {
    port: 5173,
  },
});
