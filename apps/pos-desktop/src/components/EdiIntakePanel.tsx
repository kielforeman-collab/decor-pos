// EDI Intake, 832 Price-Application Review & Vendor Catalog CSV Import
// (absorbed from web-backoffice into OrdersScreen).

import { useState } from 'react'
import {
  getPriceApplication,
  importEdiCatalog,
  intakeEdi,
  markPriceApplicationApplied,
} from '../backofficeApi'
import type {
  CatalogImportResponse,
  IntakeResponse,
  PriceCatalogApplicationRow,
} from '../backofficeTypes'

export default function EdiIntakePanel({
  selectedPoId,
  defaultVendorId,
  onToast,
}: {
  selectedPoId: string | null
  defaultVendorId: string
  onToast: (msg: string) => void
}) {
  const [docType, setDocType] = useState('855')
  const [vendorId, setVendorId] = useState(defaultVendorId || 'VENDOR1')
  const [rawText, setRawText] = useState('')
  const [intakeResult, setIntakeResult] = useState<IntakeResponse | null>(null)

  const [priceAppId, setPriceAppId] = useState('')
  const [priceApp, setPriceApp] = useState<PriceCatalogApplicationRow | null>(null)
  const [reviewer, setReviewer] = useState('manager')

  const [catalogCsv, setCatalogCsv] = useState('')
  const [catalogResult, setCatalogResult] = useState<CatalogImportResponse | null>(null)

  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function doIntake() {
    if (!rawText.trim() || !vendorId.trim()) {
      setError('Vendor ID and raw X12 / CSV text are required.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const res = await intakeEdi({
        doc_type: docType,
        raw_text: rawText,
        vendor_id: vendorId.trim(),
        related_po_id: selectedPoId,
      })
      setIntakeResult(res)
      if (res.price_application_id) {
        setPriceAppId(res.price_application_id)
        try {
          setPriceApp(await getPriceApplication(res.price_application_id))
        } catch {
          // optional auto-fetch
        }
      }
      onToast(`EDI ${res.doc_type} intake recorded.`)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doLoadPriceApp() {
    if (!priceAppId.trim()) return
    setBusy(true)
    setError(null)
    try {
      setPriceApp(await getPriceApplication(priceAppId.trim()))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doMarkApplied() {
    if (!priceApp || !reviewer.trim()) return
    setBusy(true)
    setError(null)
    try {
      const updated = await markPriceApplicationApplied(priceApp.id, reviewer.trim())
      setPriceApp(updated)
      onToast('832 price application marked reviewed (no product prices rewritten).')
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  async function doCatalogImport() {
    if (!catalogCsv.trim() || !vendorId.trim()) {
      setError('Vendor ID and catalog CSV text are required.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const res = await importEdiCatalog(catalogCsv, vendorId.trim())
      setCatalogResult(res)
      onToast(`Imported ${res.imported} vendor catalog item(s).`)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="screen-panel">
      <h3>EDI intake, 832 price review &amp; vendor catalog CSV</h3>
      <p className="screen-note" style={{ marginBottom: 8 }}>
        Paste vendor X12 (855 / 856 / 810 / 832) or CSV. An 855 reports ack variances without mutating the PO; an 832 stays review-only until marked applied.
      </p>

      {error && (
        <div className="error" role="alert">
          <span>{error}</span>
          <button type="button" className="banner-x" aria-label="Dismiss error" onClick={() => setError(null)}>✕</button>
        </div>
      )}

      <div className="screen-form-row">
        <label>
          Doc type
          <select className="screen-input" value={docType} onChange={(e) => setDocType(e.target.value)}>
            <option value="855">855 PO Ack</option>
            <option value="856">856 ASN</option>
            <option value="810">810 Invoice</option>
            <option value="832">832 Price Catalog</option>
          </select>
        </label>
        <label>
          Vendor ID
          <input className="screen-input mono" value={vendorId} onChange={(e) => setVendorId(e.target.value)} />
        </label>
        <button type="button" className="screen-btn screen-btn-primary" disabled={busy} onClick={() => void doIntake()}>
          Intake document
        </button>
      </div>
      <textarea
        className="screen-textarea"
        placeholder="Paste X12 document…"
        value={rawText}
        onChange={(e) => setRawText(e.target.value)}
      />
      {intakeResult && (
        <pre className="screen-pre">{JSON.stringify(intakeResult, null, 2)}</pre>
      )}

      <div className="screen-sub">
        <h4>832 Price-catalog application review</h4>
        <div className="screen-form-row">
          <label>
            Application ID (UUID)
            <input className="screen-input mono" value={priceAppId} onChange={(e) => setPriceAppId(e.target.value)} placeholder="UUID" />
          </label>
          <button type="button" className="screen-btn" disabled={busy} onClick={() => void doLoadPriceApp()}>
            Load
          </button>
          <label>
            Reviewed by
            <input className="screen-input" value={reviewer} onChange={(e) => setReviewer(e.target.value)} />
          </label>
          <button type="button" className="screen-btn screen-btn-primary" disabled={busy || !priceApp} onClick={() => void doMarkApplied()}>
            Mark applied
          </button>
        </div>
        {priceApp && (
          <pre className="screen-pre">{JSON.stringify(priceApp, null, 2)}</pre>
        )}
      </div>

      <div className="screen-sub">
        <h4>Vendor catalog CSV import</h4>
        <textarea
          className="screen-textarea"
          placeholder="vendor_sku,description,cost,list_price,uom,barcode"
          value={catalogCsv}
          onChange={(e) => setCatalogCsv(e.target.value)}
        />
        <div className="screen-actions" style={{ marginTop: 6 }}>
          <button type="button" className="screen-btn" disabled={busy} onClick={() => void doCatalogImport()}>
            Import catalog CSV
          </button>
        </div>
        {catalogResult && (
          <p className="screen-meta">
            Imported {catalogResult.imported} item(s) · {catalogResult.errors.length} error(s).
          </p>
        )}
      </div>
    </div>
  )
}
