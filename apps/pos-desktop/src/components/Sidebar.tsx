interface SidebarProps {
  activeNav: string
  deadLetterCount?: number
  userRole?: string
  onNavigate: (name: string) => void
  onChangeLocation: () => void
  onSwitchOperator: () => void
  onSignOut: () => void
}

const GROUPS: { title: string; items: { name: string; roles?: string[] }[] }[] = [
  {
    title: 'Point of sale',
    items: [{ name: 'Transactions' }, { name: 'Deliveries' }],
  },
  {
    title: 'Inventory',
    items: [{ name: 'Orders' }, { name: 'Transfers' }, { name: 'Counts' }],
  },
  {
    title: 'Store',
    items: [
      { name: 'Products' },
      { name: 'Customers' },
      { name: 'CRM' },
      { name: 'Vendors' },
      { name: 'Scheduling' },
    ],
  },
  {
    title: 'Analytics',
    items: [{ name: 'Dashboards' }, { name: 'Reports' }],
  },
  {
    title: 'System',
    items: [
      { name: 'Sync' },
      { name: 'Admin', roles: ['Admin', 'Manager'] },
    ],
  },
]

export default function Sidebar({
  activeNav,
  deadLetterCount = 0,
  userRole,
  onNavigate,
  onChangeLocation,
  onSwitchOperator,
  onSignOut,
}: SidebarProps) {
  return (
    <nav className="sidebar" aria-label="Sections">
      {GROUPS.map((g) => {
        const visible = g.items.filter(
          (item) => !item.roles || !userRole || item.roles.includes(userRole),
        )
        if (visible.length === 0) return null
        return (
          <div key={g.title} className="nav-group">
            <div className="nav-group-title">{g.title}</div>
            {visible.map((item) => {
              const badge =
                item.name === 'Sync' && deadLetterCount > 0 ? String(deadLetterCount) : undefined
              return (
                <button
                  key={item.name}
                  type="button"
                  className={`nav-item${activeNav === item.name ? ' active' : ''}`}
                  aria-current={activeNav === item.name ? 'page' : undefined}
                  onClick={() => onNavigate(item.name)}
                >
                  <span>{item.name}</span>
                  {badge && <span className="nav-badge" title={`${badge} dead-lettered sync entries`}>{badge}</span>}
                </button>
              )
            })}
          </div>
        )
      })}
      <div className="side-foot">
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
