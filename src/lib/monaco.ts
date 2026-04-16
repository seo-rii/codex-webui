import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import jsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";
import cssWorker from "monaco-editor/esm/vs/language/css/css.worker?worker";
import htmlWorker from "monaco-editor/esm/vs/language/html/html.worker?worker";
import tsWorker from "monaco-editor/esm/vs/language/typescript/ts.worker?worker";

let monacoPromise: Promise<typeof import("monaco-editor")> | null = null;

export async function loadMonaco() {
  if (!monacoPromise) {
    monacoPromise = import("monaco-editor").then((monaco) => {
      const target = globalThis as typeof globalThis & {
        MonacoEnvironment?: {
          getWorker: (_workerId: string, label: string) => Worker;
        };
      };

      target.MonacoEnvironment = {
        getWorker: (_workerId: string, label: string) => {
          if (label === "json") {
            return new jsonWorker();
          }
          if (label === "css" || label === "scss" || label === "less") {
            return new cssWorker();
          }
          if (label === "html" || label === "handlebars" || label === "razor") {
            return new htmlWorker();
          }
          if (label === "typescript" || label === "javascript") {
            return new tsWorker();
          }
          return new editorWorker();
        }
      };

      return monaco;
    });
  }

  return monacoPromise;
}
