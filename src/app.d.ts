declare global {
  namespace App {
    interface Locals {
      authenticated: boolean;
      locale: string;
      profileId: string;
      textDirection: "ltr" | "rtl";
    }
  }
}

export {};
