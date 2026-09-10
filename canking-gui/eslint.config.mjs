import { defineConfig } from "eslint/config";
import eslintPluginReact from "eslint-plugin-react";
import eslintPluginReactHooks from "eslint-plugin-react-hooks";
import eslintPluginReactRefresh from "eslint-plugin-react-refresh";
import eslintPluginTs from "@typescript-eslint/eslint-plugin";
import tsParser from "@typescript-eslint/parser";

export default defineConfig(
  { ignores: ["**/node_modules", "**/dist", "**/out", "**/*.js"] },
  eslintPluginReact.configs.flat.recommended,
  eslintPluginReact.configs.flat["jsx-runtime"],
  {
    settings: {
      react: {
        version: "detect",
      },
    },
  },
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        ecmaVersion: "latest",
        sourceType: "module",
        ecmaFeatures: {
          jsx: true,
        },
      },
    },
    plugins: {
      "@typescript-eslint": eslintPluginTs,
      "react-hooks": eslintPluginReactHooks,
      "react-refresh": eslintPluginReactRefresh,
    },
    rules: {
      ...eslintPluginTs.configs.recommended.rules,
      ...eslintPluginReactHooks.configs.recommended.rules,
      ...eslintPluginReactRefresh.configs.vite.rules,
      "react-hooks/set-state-in-effect": [
        "error",
        {
          environment: {
            enableTreatSetIdentifiersAsStateSetters: true,
          },
        },
      ],
      "react-hooks/set-state-in-render": [
        "error",
        {
          environment: {
            enableTreatSetIdentifiersAsStateSetters: true,
          },
        },
      ],
    },
  },
  {
    files: ["src/main.tsx"],
    rules: {
      "react-refresh/only-export-components": "off",
    },
  },
);
