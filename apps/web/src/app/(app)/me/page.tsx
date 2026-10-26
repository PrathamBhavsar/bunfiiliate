"use client";

import { Banknote, Clock, Wallet } from "lucide-react";
import { StatCard } from "@/components/stat-card";

// Earnings come from the affiliate portal API (T34). Until then every card reads "—".
export default function MeDashboardPage() {
  return (
    <section aria-labelledby="page-title" className="grid gap-3">
      <header>
        <h1 id="page-title" className="text-base font-semibold">
          Dashboard
        </h1>
        <p className="text-xs text-muted">Placeholder page. Your earnings land in T34.</p>
      </header>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <StatCard label="Pending" value="—" icon={Clock} />
        <StatCard label="Payable" value="—" icon={Wallet} accent />
        <StatCard label="Paid" value="—" icon={Banknote} />
      </div>
    </section>
  );
}
