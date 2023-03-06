import { defineConfig } from 'vite';

export default defineConfig(async () => {
  return {
    build: {
      lib: {
        entry: './src/index.ts',
        fileName: 'woodland-core',
        formats: ['es', 'cjs'],
      },
      rollupOptions: {
        output: {
          exports: 'named',
        },
      },
    },
  };
});
