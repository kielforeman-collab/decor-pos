import { useEffect, useRef } from 'react'
import { TENDER_LABELS, fmtMoney, type InvoiceData } from '../types'

interface InvoiceViewProps {
  invoice: InvoiceData
  reprint: boolean
  autoEmail: boolean
  onClose: () => void
  onNewSale: () => void
  onToast: (msg: string) => void
}

function money(m: { amount: string; currency: 'Usd' | 'Cad' } | null | undefined): string {
  return fmtMoney(m)
}

/** Letter invoice — the sheet stays WHITE PAPER in every theme. */
export default function InvoiceView({ invoice, reprint, autoEmail, onClose, onNewSale, onToast }: InvoiceViewProps) {
  const printBtnRef = useRef<HTMLButtonElement>(null)
  const emailedRef = useRef(false)

  useEffect(() => {
    printBtnRef.current?.focus()
  }, [])

  const email = invoice.customer?.email ?? null
  const subject = `Invoice ${invoice.doc_number} — North Shore Decorating Centre — ${money(invoice.totals.grand_total)}`
  const bodyLines = [
    `Invoice ${invoice.doc_number}`,
    `Date: ${new Date(invoice.created_at).toLocaleString('en-CA')}`,
    `Total due: ${money(invoice.totals.grand_total)}`,
    '',
    'Thank you for shopping at North Shore Decorating Centre.',
  ]
  const mailto = email
    ? `mailto:${encodeURIComponent(email)}?subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(bodyLines.join('\n'))}`
    : null

  // Contractor accounts with emailed invoices ticked get the compose
  // window automatically, 700ms after the invoice opens.
  useEffect(() => {
    if (!autoEmail || !mailto || emailedRef.current) return
    emailedRef.current = true
    const t = window.setTimeout(() => {
      window.location.href = mailto
    }, 700)
    return () => window.clearTimeout(t)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoEmail, mailto])

  const doPrint = () => window.print()
  const doEmail = () => {
    if (!mailto) {
      onToast('No email address on file for this customer — type an address to email the invoice.')
      return
    }
    window.location.href = mailto
  }
  // No PDF library ships in the webview and a hand-rolled
  // %PDF-1.4 blob is deliberately NOT reproduced: the print dialog's
  // "Save as PDF" produces the real letter PDF from this same sheet.
  const doPdf = () => {
    onToast('Choose “Save as PDF” in the print dialog to download the letter invoice as PDF.')
    window.print()
  }

  const t = invoice.totals
  const charge = t.charge
  const cashTender = invoice.tenders.find((x) => x.kind === 'Cash')
  const changeGiven = cashTender?.tendered && cashTender?.change ? cashTender.change : null

  return (
    <div className="modal-overlay invoice-overlay" role="dialog" aria-modal="true" aria-label="Invoice">
      <div className="modal invoice-modal">
        <div className="modal-head">
          <h2>Invoice{reprint ? ' — Copy REPRINT' : ''}</h2>
          <span className="total-chip">LETTER · 8.5 × 11 IN</span>
          <button type="button" className="modal-x" onClick={onClose} aria-label="Close invoice">✕</button>
        </div>

        <div className="sheet mono" id="invoice-sheet">
          <div className="sheet-head">
            <div className="sheet-store">NORTH SHORE DECORATING CENTRE</div>
            <div className="sheet-sub">Benjamin Moore dealer · Main counter · Register 1</div>
            <div className="sheet-sub">Prices from price book 10005510 · Effective July 17, 2026</div>
            {reprint && <div className="sheet-reprint">COPY — REPRINT</div>}
          </div>
          <div className="sheet-meta">
            <div><span>INVOICE</span><strong>{invoice.doc_number}</strong></div>
            <div><span>Date</span><strong>{new Date(invoice.created_at).toLocaleString('en-CA')}</strong></div>
            {invoice.sale_number && <div><span>Sale #</span><strong>{invoice.sale_number}</strong></div>}
            {invoice.clerk && <div><span>Clerk</span><strong>{invoice.clerk}</strong></div>}
            {invoice.terms && <div><span>Terms</span><strong>{invoice.terms}</strong></div>}
          </div>
          <div className="sheet-parties">
            <div>
              <span>BILL TO</span>
              <strong>{invoice.customer?.name ?? 'Walk-in'}</strong>
              {invoice.customer?.account_number && <div>{invoice.customer.account_number}</div>}
              {invoice.customer?.email && <div>{invoice.customer.email}</div>}
            </div>
            <div>
              <span>JOB &amp; FULFILMENT</span>
              <strong>{invoice.job_name ?? '—'}</strong>
              {invoice.po_number && <div>PO {invoice.po_number}</div>}
              {invoice.fulfilment_mode && <div>{invoice.fulfilment_mode}</div>}
            </div>
          </div>
          <table className="sheet-table">
            <thead>
              <tr><th>#</th><th>SKU</th><th>Description</th><th>Qty</th><th>UOM</th><th>Unit price</th><th>Disc</th><th>Amount</th></tr>
            </thead>
            <tbody>
              {invoice.lines.map((l, i) => (
                <tr key={`${l.sku}-${i}`}>
                  <td>{i + 1}</td>
                  <td>{l.sku}</td>
                  <td>
                    {l.description}
                    {l.size_label && <span className="sheet-size"> · {l.size_label}</span>}
                    {l.tint && <div className="sheet-tint">TINT: {l.tint}</div>}
                  </td>
                  <td>{l.qty}</td>
                  <td>{l.uom}</td>
                  <td>{money(l.unit_price)}</td>
                  <td>{l.disc_pct && Number(l.disc_pct) > 0 ? `${l.disc_pct}%` : '—'}</td>
                  <td>{money(l.amount)}</td>
                </tr>
              ))}
            </tbody>
          </table>

          <div className="sheet-lower">
            <div className="sheet-pay">
              <strong>Payment</strong>
              {reprint ? (
                <div>Paid — original sale</div>
              ) : invoice.tenders.length === 0 ? (
                <div>—</div>
              ) : (
                invoice.tenders.map((x, i) => (
                  <div key={i}>{TENDER_LABELS[x.kind] ?? x.kind} — {money(x.amount)}{x.reference ? ` (${x.reference})` : ''}</div>
                ))
              )}
              {cashTender?.tendered && <div>Tendered {money(cashTender.tendered)}</div>}
              {changeGiven && <div>Change given {money(changeGiven)}</div>}
              {charge && (
                <>
                  <div>Previous balance {money(charge.previous_balance)}</div>
                  <div>Charged {money(charge.charged)}</div>
                  <div>New balance {money(charge.new_balance)}</div>
                  <div>Credit limit {money(charge.credit_limit)}</div>
                  <div>Available credit {money(charge.available_credit)}</div>
                </>
              )}
              <div className="sheet-sign">Signature ________________________________</div>
            </div>
            <div className="sheet-totals">
              <div><span>Subtotal</span><strong>{money(t.subtotal)}</strong></div>
              {Number(t.discounts.amount) !== 0 && <div><span>Discounts</span><strong>−{money(t.discounts)}</strong></div>}
              {Number(t.eco_fee_total.amount) !== 0 && <div><span>Eco fees</span><strong>{money(t.eco_fee_total)}</strong></div>}
              {Number(t.delivery.amount) !== 0 && <div><span>Delivery</span><strong>{money(t.delivery)}</strong></div>}
              <div><span>GST 5%</span><strong>{money(t.gst)}</strong></div>
              <div><span>PST 7%</span><strong>{money(t.pst)}</strong></div>
              <div className="sheet-grand"><span>{reprint ? 'Balance due' : 'TOTAL DUE'}</span><strong>{reprint ? '$0.00' : money(t.grand_total)}</strong></div>
            </div>
          </div>

          {invoice.note && <div className="sheet-note">Note: {invoice.note}</div>}
          <div className="sheet-foot">
            <div>Tinted paint is final sale. Returns within 30 days with receipt; untinted stock only.</div>
            <div className="sheet-barcode" aria-hidden="true">{invoice.doc_number.split('').map((c) => (c.charCodeAt(0) % 2 ? '▮' : '▯')).join('')}</div>
            <div>{invoice.doc_number} · THANK YOU</div>
          </div>
        </div>

        <div className="modal-foot invoice-foot">
          <button type="button" ref={printBtnRef} onClick={doPrint}>Print invoice</button>
          <button type="button" onClick={doEmail}>Email invoice</button>
          <button type="button" onClick={doPdf}>Download PDF</button>
          <button type="button" onClick={onClose}>Close</button>
          <button type="button" className="new-sale-btn" onClick={onNewSale}>New sale</button>
        </div>
      </div>
    </div>
  )
}
