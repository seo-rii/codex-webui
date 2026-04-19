declare global {
  namespace App {
    interface Locals {
      authenticated: boolean;
      locale: string;
      profileId: string;
      textDirection: "ltr" | "rtl";
    }
  }

  interface Window {
    hcaptcha?: {
      render: (
        container: HTMLElement,
        options: {
          sitekey: string;
          callback?: (token: string) => void;
          "expired-callback"?: () => void;
          "error-callback"?: () => void;
          theme?: "light" | "dark";
        }
      ) => string | number;
      reset?: (widgetId?: string | number) => void;
      remove?: (widgetId?: string | number) => void;
    };
  }
}

export {};
