// Hugeicons (free, MIT). Curated set: add a name here before using it in brand.js.
import {
  Copy01Icon, Tick02Icon, GithubIcon, ArrowRight01Icon, ArrowUpRight01Icon, Download01Icon,
  CommandLineIcon, BookOpen01Icon, Search01Icon, Menu01Icon, Cancel01Icon, Timer01Icon,
  Database01Icon, ShieldKeyIcon, Wallet01Icon, Coins01Icon, Link01Icon, FlashIcon, Brain01Icon,
  UserGroupIcon, Mail01Icon, Attachment01Icon, Notebook01Icon, GitBranchIcon, Calendar01Icon,
  Mailbox01Icon, Message01Icon, WebhookIcon, LockKeyIcon, ServerStack01Icon, PlugSocketIcon,
  Layers01Icon, Analytics01Icon, ChartLineData01Icon, FingerPrintIcon, Alert02Icon, FilterIcon,
  Money01Icon, Invoice01Icon, CreditCardIcon, Rocket01Icon, CheckListIcon, Tree01Icon, Share01Icon,
  HardDriveIcon, Package01Icon, Settings01Icon, KeyboardIcon, Globe02Icon, CodeIcon, Refresh01Icon,
  RepeatIcon, Idea01Icon, PuzzleIcon, Image01Icon, File01Icon, Pdf01Icon, BubbleChatIcon, EyeIcon,
  Bug01Icon, Activity01Icon, Clock01Icon, HourglassIcon, Fire02Icon, Shield01Icon, Folder01Icon,
  SourceCodeIcon, StarIcon, DashboardSquare01Icon, CloudServerIcon, DatabaseSyncIcon,
  FileExportIcon, MailSend01Icon, InboxIcon, Ticket01Icon, CustomerSupportIcon, User02Icon,
} from "@hugeicons/core-free-icons";

const set = {
  copy: Copy01Icon, check: Tick02Icon, github: GithubIcon, arrow: ArrowRight01Icon, out: ArrowUpRight01Icon,
  download: Download01Icon, terminal: CommandLineIcon, book: BookOpen01Icon, search: Search01Icon,
  menu: Menu01Icon, close: Cancel01Icon, timer: Timer01Icon, db: Database01Icon, shield: ShieldKeyIcon,
  wallet: Wallet01Icon, coins: Coins01Icon, link: Link01Icon, flash: FlashIcon, brain: Brain01Icon,
  users: UserGroupIcon, mail: Mail01Icon, attach: Attachment01Icon, notebook: Notebook01Icon,
  branch: GitBranchIcon, calendar: Calendar01Icon, mailbox: Mailbox01Icon, message: Message01Icon,
  webhook: WebhookIcon, lock: LockKeyIcon, server: ServerStack01Icon, plug: PlugSocketIcon,
  layers: Layers01Icon, analytics: Analytics01Icon, chart: ChartLineData01Icon, finger: FingerPrintIcon,
  alert: Alert02Icon, filter: FilterIcon, money: Money01Icon, invoice: Invoice01Icon, card: CreditCardIcon,
  rocket: Rocket01Icon, checklist: CheckListIcon, tree: Tree01Icon, share: Share01Icon, disk: HardDriveIcon,
  package: Package01Icon, settings: Settings01Icon, keyboard: KeyboardIcon, globe: Globe02Icon,
  code: CodeIcon, refresh: Refresh01Icon, repeat: RepeatIcon, idea: Idea01Icon, puzzle: PuzzleIcon,
  image: Image01Icon, file: File01Icon, pdf: Pdf01Icon, chat: BubbleChatIcon, eye: EyeIcon, bug: Bug01Icon,
  activity: Activity01Icon, clock: Clock01Icon, hourglass: HourglassIcon, fire: Fire02Icon,
  shield2: Shield01Icon, folder: Folder01Icon, source: SourceCodeIcon, star: StarIcon,
  dash: DashboardSquare01Icon, cloud: CloudServerIcon, sync: DatabaseSyncIcon, export: FileExportIcon,
  send: MailSend01Icon, inbox: InboxIcon, ticket: Ticket01Icon, support: CustomerSupportIcon, user: User02Icon,
};

export function icon(name, cls = "icon") {
  const data = set[name];
  if (!data) throw new Error(`unknown icon: ${name}`);
  const body = data.map(([tag, a]) => {
    const attrs = Object.entries(a).filter(([k]) => k !== "key")
      .map(([k, v]) => `${k.replace(/[A-Z]/g, (m) => "-" + m.toLowerCase())}="${v}"`).join(" ");
    return `<${tag} ${attrs}/>`;
  }).join("");
  return `<svg class="${cls}" viewBox="0 0 24 24" fill="none" aria-hidden="true">${body}</svg>`;
}
