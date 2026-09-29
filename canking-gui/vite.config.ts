import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import thirdPartyLicenses from "./third-party-licenses";

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react(), thirdPartyLicenses("third-party-npm.json")],
  build: {
    rolldownOptions: {
      output: {
        codeSplitting: {
          minSize: 30 * 1024,
          maxSize: 450 * 1024,
          minShareCount: 2,
          groups: [
            {
              name: "vendor-react",
              test: /node_modules[\\/](react|react-dom|react-router|react-router-dom)[\\/]/,
              priority: 40,
            },
            {
              name: "vendor-ui",
              test: /node_modules[\\/](@mui|@emotion)[\\/]/,
              priority: 30,
              maxSize: 2 * 1024 * 1024,
            },
            {
              name: "vendor-canking",
              test: /node_modules[\\/]@kvaser[\\/]canking-api[\\/]/,
              priority: 20,
              maxSize: 2 * 1024 * 1024,
            },
            {
              name: "vendor-protobuf",
              test: /node_modules[\\/](google-protobuf|long)[\\/]/,
              priority: 15,
            },
            {
              name: "vendor-misc",
              test: /node_modules[\\/]/,
              priority: 1,
            },
          ],
        },
      },
    },
  },
});
