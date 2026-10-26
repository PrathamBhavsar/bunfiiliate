"use client";

import { Banknote, MousePointerClick, ReceiptText, Wallet } from "lucide-react";
import { StatCard } from "@/components/stat-card";

// Headline numbers arrive with the analytics API (T33). Until then every card reads "—".
export default function DashboardPage() {
  return (
    <section aria-labelledby="page-title" className="grid gap-3">
      <header>
        <h1 id="page-title" className="text-base font-semibold">
          Dashboard
        </h1>
        <p className="text-xs text-muted">Placeholder page. Live figures land in T33.</p>
      </header>
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <StatCard label="Clicks (30d)" value="—" icon={MousePointerClick} />
        <StatCard label="Conversions (30d)" value="—" icon={ReceiptText} />
        <StatCard label="Revenue (30d)" value="—" icon={Banknote} />
        <StatCard label="Commission payable" value="—" icon={Wallet} accent />
      </div>
    </section>
  );
}
