// Vendors — vendor list, contact detail (editable), EDI status per
// purchase order and transport records. No contact details, prices
// or catalogs are invented — empty fields stay empty.

import { useCallback, useEffect, useState } from 'react'
import {
  createVendor,
  getVendor,
  listEdiDocuments,
  listTransportRecords,
  listVendors,
  updateVendor,
  type EdiDocumentRow,
  type PurchaseOrderView,
  type TransportRecord,
  type Vendor,
  type VendorDetail,
} from '../api'

const EDI_TYPE_LABELS: Record<string, string> = {
  Po850: '850 PO',
  Ack855: '855 ack',
  Asn856: '856 ASN',
  Invoice810: '810 invoice',
  Catalog832: '832 catalog',
}

export default function VendorsScreen({
  storeId,
  onToast,
}: {
  storeId: string | null
  onToast: (msg: string) => void
}) {
  const [vendors, setVendors] = useState<Vendor[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detail, setDetail] = useState<VendorDetail | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [creating, setCreating] = useState(false)
  const [newName, setNewName] = useState('')
  const [newDisplayName, setNewDisplayName] = useState('')
  const [newContact, setNewContact] = useState('')
  const [newEmail, setNewEmail] = useState('')
  const [newPhone, setNewPhone] = useState('')
  const [newEdi, setNewEdi] = useState(false)
  const [busy, setBusy] = useState(false)

  const [displayName, setDisplayName] = useState('')
  const [contactName, setContactName] = useState('')
  const [email, setEmail] = useState('')
  const [phone, setPhone] = useState('')
  const [ediEnabled, setEdiEnabled] = useState(false)
  const [saving, setSaving] = useState(false)

  const [selectedPoId, setSelectedPoId] = useState<string | null>(null)
  const [ediDocs, setEdiDocs] = useState<EdiDocumentRow[] | null>(null)
  const [ediBusy, setEdiBusy] = useState(false)
  const [transport, setTransport] = useState<TransportRecord[] | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const list = await listVendors()
      setVendors(list)
      if (selectedId && !list.some((v) => v.id === selectedId)) {
        setSelectedId(null)
        setDetail(null)
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [selectedId])

  useEffect(() => {
    void load()
  }, [load])

  // Load the vendor detail (with its POs) and transport records.
  useEffect(() => {
    if (!selectedId) {
      setDetail(null)
      setTransport(null)
      setSelectedPoId(null)
      setEdiDocs(null)
      return
    }
    let cancelled = false
    getVendor(selectedId)
      .then((d) => {
        if (cancelled || !d) return
        setDetail(d)
        setDisplayName(d.vendor.display_name ?? '')
        setContactName(d.vendor.contact_name ?? '')
        setEmail(d.vendor.email ?? '')
        setPhone(d.vendor.phone ?? '')
        setEdiEnabled(d.vendor.edi_enabled)
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
    listTransportRecords(selectedId)
      .then((rows) => {
        if (!cancelled) setTransport(rows)
      })
      .catch(() => {
        if (!cancelled) setTransport([])
      })
    setSelectedPoId(null)
    setEdiDocs(null)
    return () => {
      cancelled = true
    }
  }, [selectedId])

  async function doCreate() {
    if (!newName.trim()) {
      setError('Vendor name is required.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const created = await createVendor({
        name: newName.trim(),
        display_name: newDisplayName.trim() || null,
        contact_name: newContact.trim() || null,
        email: newEmail.trim() || null,
        phone: newPhone.trim() || null,
        edi_enabled: newEdi,
      })
      setCreating(false)
      setNewName('')
      setNewDisplayName('')
      setNewContact('')
      setNewEmail('')
      setNewPhone('')
      setNewEdi(false)
      await load()
      setSelectedId(created.id)
      onToast(`Vendor ${created.name} created.`)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doSave() {
    if (!detail) return
    setSaving(true)
    setError(null)
    try {
      const updated = await updateVendor(detail.vendor.id, {
        display_name: displayName.trim() || null,
        contact_name: contactName.trim() || null,
        email: email.trim() || null,
        phone: phone.trim() || null,
        edi_enabled: ediEnabled,
      })
      setDetail((prev) => (prev ? { ...prev, vendor: updated } : prev))
      setVendors((prev) => prev.map((v) => (v.id === updated.id ? updated : v)))
      onToast(`Vendor ${updated.name} saved.`)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  async function loadEdiDocs(po: PurchaseOrderView) {
    setSelectedPoId(po.id)
    setEdiBusy(true)
    setError(null)
    try {
      setEdiDocs(await listEdiDocuments(po.id))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setEdiDocs(null)
    } finally {
      setEdiBusy(false)
    }
  }

  if (!storeId) return <section className="screen"><p className="screen-empty">Loading store…</p></section>

  const vendor = detail?.vendor ?? null

  return (
    <section className="screen" aria-label="Vendors">
      <div className="screen-head">
        <h2>Vendors</h2>
        <div className="screen-actions">
          <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => setCreating((c) => !c)}>
            {creating ? 'Close new vendor' : 'New vendor'}
          </button>
          <button type="button" className="screen-btn" disabled={loading} onClick={() => void load()}>
            Refresh
          </button>
        </div>
      </div>

      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
          <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>✕</button>
        </div>
      )}

      {creating && (
        <div className="screen-panel">
          <h3>New vendor</h3>
          <div className="screen-form-row">
            <label>
              Name *
              <input type="text" className="screen-input" value={newName} placeholder="Vendor name" onChange={(e) => setNewName(e.target.value)} />
            </label>
            <label>
              Display name
              <input type="text" className="screen-input" value={newDisplayName} onChange={(e) => setNewDisplayName(e.target.value)} />
            </label>
            <label>
              Contact name
              <input type="text" className="screen-input" value={newContact} onChange={(e) => setNewContact(e.target.value)} />
            </label>
            <label>
              Email
              <input type="email" className="screen-input" value={newEmail} onChange={(e) => setNewEmail(e.target.value)} />
            </label>
            <label>
              Phone
              <input type="tel" className="screen-input" value={newPhone} onChange={(e) => setNewPhone(e.target.value)} />
            </label>
            <label>
              EDI enabled
              <select className="screen-input" value={newEdi ? 'yes' : 'no'} onChange={(e) => setNewEdi(e.target.value === 'yes')}>
                <option value="no">No</option>
                <option value="yes">Yes</option>
              </select>
            </label>
          </div>
          <div className="screen-actions">
            <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doCreate()}>
              Create vendor
            </button>
          </div>
        </div>
      )}

      <div className="screen-split">
        <div className="screen-panel">
          <h3>Vendors</h3>
          {loading && vendors.length === 0 ? (
            <p className="screen-empty">Loading…</p>
          ) : vendors.length === 0 ? (
            <p className="screen-empty">No vendors yet — create one above.</p>
          ) : (
            <table className="screen-table screen-table-clickable">
              <thead>
                <tr>
                  <th>Name</th>
                  <th className="num">POs</th>
                  <th className="num">Open</th>
                  <th>EDI</th>
                </tr>
              </thead>
              <tbody>
                {vendors.map((v) => (
                  <tr
                    key={v.id}
                    className={v.id === selectedId ? 'selected' : ''}
                    onClick={() => setSelectedId(v.id)}
                  >
                    <td>{v.display_name || v.name}</td>
                    <td className="num">{v.po_count}</td>
                    <td className="num">{v.open_po_count}</td>
                    <td>{v.edi_enabled ? 'Yes' : 'No'}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        <div className="screen-panel">
          <h3>Vendor detail</h3>
          {!vendor ? (
            <p className="screen-empty">Pick a vendor on the left.</p>
          ) : (
            <>
              <p className="screen-meta">
                <span className="mono">{vendor.name}</span>
                {vendor.last_po_at ? ` · Last PO ${new Date(vendor.last_po_at).toLocaleDateString()}` : ' · No POs yet'}
              </p>
              <div className="screen-form-row">
                <label>
                  Display name
                  <input type="text" className="screen-input" value={displayName} placeholder={vendor.name} onChange={(e) => setDisplayName(e.target.value)} />
                </label>
                <label>
                  Contact name
                  <input type="text" className="screen-input" value={contactName} placeholder="—" onChange={(e) => setContactName(e.target.value)} />
                </label>
                <label>
                  Email
                  <input type="email" className="screen-input" value={email} placeholder="—" onChange={(e) => setEmail(e.target.value)} />
                </label>
                <label>
                  Phone
                  <input type="tel" className="screen-input" value={phone} placeholder="—" onChange={(e) => setPhone(e.target.value)} />
                </label>
                <label>
                  EDI enabled
                  <select className="screen-input" value={ediEnabled ? 'yes' : 'no'} onChange={(e) => setEdiEnabled(e.target.value === 'yes')}>
                    <option value="no">No</option>
                    <option value="yes">Yes</option>
                  </select>
                </label>
              </div>
              <div className="screen-actions">
                <button type="button" className="screen-btn screen-btn-primary" disabled={saving} onClick={() => void doSave()}>
                  Save vendor
                </button>
              </div>

              <div className="screen-sub">
                <h4>Purchase orders</h4>
                {!detail || detail.purchase_orders.length === 0 ? (
                  <p className="screen-empty">No purchase orders for this vendor yet.</p>
                ) : (
                  <table className="screen-table screen-table-clickable">
                    <thead>
                      <tr>
                        <th>PO</th>
                        <th>Status</th>
                        <th className="num">Lines</th>
                        <th>Created</th>
                      </tr>
                    </thead>
                    <tbody>
                      {detail.purchase_orders.map((p) => (
                        <tr
                          key={p.id}
                          className={p.id === selectedPoId ? 'selected' : ''}
                          onClick={() => void loadEdiDocs(p)}
                        >
                          <td className="mono">{p.po_number}</td>
                          <td>{p.status}</td>
                          <td className="num">{p.lines.length}</td>
                          <td>{new Date(p.created_at).toLocaleDateString()}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
                {selectedPoId && (
                  <div className="screen-sub">
                    <h4>EDI status — pick a PO above to see its documents</h4>
                    {ediBusy ? (
                      <p className="screen-empty">Loading…</p>
                    ) : ediDocs === null ? (
                      <p className="screen-empty">No EDI documents loaded for this order yet.</p>
                    ) : ediDocs.length === 0 ? (
                      <p className="screen-empty">No EDI documents for this order yet.</p>
                    ) : (
                      <table className="screen-table">
                        <thead>
                          <tr>
                            <th>Type</th>
                            <th>Direction</th>
                            <th>Received</th>
                          </tr>
                        </thead>
                        <tbody>
                          {ediDocs.map((d) => (
                            <tr key={d.id}>
                              <td>{EDI_TYPE_LABELS[d.doc_type] ?? d.doc_type}</td>
                              <td>{d.direction === 'Outbound' ? 'Out' : 'In'}</td>
                              <td>{new Date(d.received_at).toLocaleString()}</td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    )}
                  </div>
                )}
              </div>

              <div className="screen-sub">
                <h4>Transport records</h4>
                {transport === null ? (
                  <p className="screen-empty">Loading…</p>
                ) : transport.length === 0 ? (
                  <p className="screen-empty">No transport records for this vendor yet.</p>
                ) : (
                  <table className="screen-table">
                    <thead>
                      <tr>
                        <th>File</th>
                        <th>Transport</th>
                        <th>Direction</th>
                        <th>Status</th>
                        <th>Created</th>
                      </tr>
                    </thead>
                    <tbody>
                      {transport.map((t) => (
                        <tr key={t.id}>
                          <td className="mono">{t.file_name}</td>
                          <td>{t.transport}</td>
                          <td>{t.direction}</td>
                          <td>{t.status}{t.error ? ` — ${t.error}` : ''}</td>
                          <td>{new Date(t.created_at).toLocaleDateString()}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            </>
          )}
        </div>
      </div>
    </section>
  )
}
