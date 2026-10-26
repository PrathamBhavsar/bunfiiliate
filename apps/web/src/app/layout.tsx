import type { Metadata, Viewport } from "next";
import type { ReactNode } from "react";
import { Providers } from "@/components/providers";
import { THEME_KEY } from "@/components/theme-toggle";
import "./globals.css";

export const metadata: Metadata = {
  title: { default: "bunfiiliate", template: "%s · bunfiiliate" },
  description: "Affiliate tracking and commissions.",
};

export const viewport: Viewport = { width: "device-width", initialScale: 1 };

// Runs before first paint so the saved (or system) theme never flashes.
const themeScript = `try{var t=localStorage.getItem(${JSON.stringify(THEME_KEY)});if(t==="dark"||(!t&&matchMedia("(prefers-color-scheme: dark)").matches))document.documentElement.classList.add("dark")}catch(e){}`;

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeScript }} />
      </head>
      <body>
        <Providers>{children}</Providers>
      </body>
    </html>
  );
}
