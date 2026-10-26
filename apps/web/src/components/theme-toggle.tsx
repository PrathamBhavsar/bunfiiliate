"use client";

import { Moon, Sun } from "lucide-react";
import { Button } from "./ui/button";

export const THEME_KEY = "bf-theme";

/** Flips `.dark` on <html> and remembers it. Icons switch on the class, so no React state is needed. */
export function ThemeToggle() {
  function toggle() {
    const dark = document.documentElement.classList.toggle("dark");
    try {
      localStorage.setItem(THEME_KEY, dark ? "dark" : "light");
    } catch {
      // Storage blocked: the theme still applies for this page load.
    }
  }
  return (
    <Button variant="ghost" size="icon" aria-label="Toggle theme" onClick={toggle}>
      <Sun className="dark:hidden" />
      <Moon className="hidden dark:block" />
    </Button>
  );
}
