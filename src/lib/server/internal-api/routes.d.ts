declare module "virtual:internal-api-routes" {
  type InternalRouteHandler = (event: {
    params: Record<string, string>;
    request: Request;
    url: URL;
    locals: {
      authenticated: boolean;
      profileId: string | null;
    };
    cookies: {
      get(name: string): string | undefined;
      set(name: string, value: string, options?: Record<string, unknown>): void;
      delete(name: string, options?: Record<string, unknown>): void;
    };
    getClientAddress?: () => string;
  }) => Response | Promise<Response>;

  type InternalRouteModule = Partial<Record<"GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS" | "HEAD", InternalRouteHandler>>;

  export const routes: Array<{
    path: string;
    pattern: RegExp;
    paramNames: string[];
    module: InternalRouteModule;
  }>;
}
