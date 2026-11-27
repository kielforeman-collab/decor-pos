interface SidebarProps {
  activeNav: string
  tillOpen: boolean
  currentThemeLabel: string
  onNavigate: (name: string) => void
  onCycleTheme: () => void
  onChangeLocation: () => void
  onSwitchOperator: () => void
  onSignOut: () => void
}

const GROUPS: { title: string; items: { name: string; badge?: string }[] }[] = [
  {
    title: 'Point of sale',
    items: [
      { name: 'Transactions' },
      { name: 'Accounts' },
      { name: 'Cash drawers' },
      { name: 'Deliveries', badge: '2' },
    ],
  },
  {
    title: 'Inventory',
    items: [{ name: 'Orders' }, { name: 'Transfers' }, { name: 'Counts' }],
  },
  {
    title: 'Store',
    items: [{ name: 'Products' }, { name: 'Customers' }, { name: 'Vendors' }],
  },
  {
    title: 'Analytics',
    items: [{ name: 'Dashboards' }, { name: 'Reports' }],
  },
]

export default function Sidebar({
  activeNav,
  tillOpen,
  currentThemeLabel,
  onNavigate,
  onCycleTheme,
  onChangeLocation,
  onSwitchOperator,
  onSignOut,
}: SidebarProps) {
  return (
    <nav className="sidebar" aria-label="Sections">
      {GROUPS.map((g) => (
        <div key={g.title} className="nav-group">
          <div className="nav-group-title">{g.title}</div>
          {g.items.map((item) => {
            let badge = item.badge
            if (item.name === 'Cash drawers') badge = tillOpen ? 'Open' : 'Closed'
            return (
              <button
                key={item.name}
                type="button"
                className={`nav-item${activeNav === item.name ? ' active' : ''}`}
                aria-current={activeNav === item.name ? 'page' : undefined}
                onClick={() => onNavigate(item.name)}
              >
                <span>{item.name}</span>
                {badge && <span className="nav-badge">{badge}</span>}
              </button>
            )
          })}
        </div>
      ))}
      <div className="side-foot">
        <button type="button" className="side-foot-btn" onClick={onCycleTheme}>
          Theme: {currentThemeLabel}
        </button>
        <button type="button" className="side-foot-btn" onClick={onChangeLocation}>
          Change location
        </button>
        <button type="button" className="side-foot-btn" onClick={onSwitchOperator}>
          Switch operator
        </button>
        <button type="button" className="side-foot-btn" onClick={onSignOut}>
          Sign out
        </button>
      </div>
    </nav>
  )
}
