import {
  Banknote,
  Bell,
  BookOpen,
  Boxes,
  ChartLine,
  FileDown,
  Inbox,
  LayoutDashboard,
  Link2,
  Mail,
  ReceiptText,
  ScrollText,
  Settings,
  UserRound,
  Users,
  Wallet,
  Webhook,
  type LucideIcon,
} from "lucide-react";
import { isAdmin, type Role } from "./auth";

export type NavItem = { href: string; label: string; icon: LucideIcon; group: string; role: "admin" | "affiliate" | "any" };

export const NAV: NavItem[] = [
  { href: "/dashboard", label: "Dashboard", icon: LayoutDashboard, group: "Workspace", role: "admin" },
  { href: "/programs", label: "Programs", icon: Boxes, group: "Workspace", role: "admin" },
  { href: "/affiliates", label: "Affiliates", icon: Users, group: "Workspace", role: "admin" },
  { href: "/customers", label: "Customers", icon: UserRound, group: "Money", role: "admin" },
  { href: "/conversions", label: "Conversions", icon: ReceiptText, group: "Money", role: "admin" },
  { href: "/ledger", label: "Ledger", icon: BookOpen, group: "Money", role: "admin" },
  { href: "/payouts", label: "Payouts", icon: Wallet, group: "Money", role: "admin" },
  { href: "/analytics", label: "Analytics", icon: ChartLine, group: "Insights", role: "admin" },
  { href: "/reports", label: "Reports", icon: FileDown, group: "Insights", role: "admin" },
  { href: "/webhooks", label: "Webhooks", icon: Webhook, group: "Operations", role: "admin" },
  { href: "/email-templates", label: "Email templates", icon: Mail, group: "Operations", role: "admin" },
  { href: "/contact-requests", label: "Contact requests", icon: Inbox, group: "Operations", role: "admin" },
  { href: "/audit-log", label: "Audit log", icon: ScrollText, group: "Operations", role: "admin" },
  { href: "/settings", label: "Settings", icon: Settings, group: "Operations", role: "admin" },
  { href: "/me", label: "Dashboard", icon: LayoutDashboard, group: "Portal", role: "affiliate" },
  { href: "/me/links", label: "Links", icon: Link2, group: "Portal", role: "affiliate" },
  { href: "/me/earnings", label: "Earnings", icon: Banknote, group: "Portal", role: "affiliate" },
  { href: "/me/payouts", label: "Payouts", icon: Wallet, group: "Portal", role: "affiliate" },
  { href: "/notifications", label: "Notifications", icon: Bell, group: "Account", role: "any" },
];

const allowed = (item: NavItem, role: Role) =>
  item.role === "any" || (item.role === "admin") === isAdmin(role);

export const navFor = (role: Role) => NAV.filter((item) => allowed(item, role));

export const homeFor = (role: Role) => (isAdmin(role) ? "/dashboard" : "/me");

export function canAccess(role: Role, pathname: string) {
  const path = pathname.length > 1 ? pathname.replace(/\/$/, "") : pathname;
  return navFor(role).some((item) => item.href === path);
}
