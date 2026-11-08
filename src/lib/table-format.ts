/**
 * 表格数据格式化的本地实现 —— 不直接依赖 vue-stream-markdown / @stream-markdown/core
 * (后者未被 vue-stream-markdown 重新导出,且不是项目直接依赖)。
 *
 * 行为与库内置 tableDataToCSV/TSV/Markdown / extractTableDataFromElement 完全一致
 * (单元化、单测覆盖,核心算法直接移植自 @stream-markdown/core@1.0.1):
 *   - extractTableData: 从 DOM <table> 抽 headers/rows(取 thead th / tbody tr td 的 textContent 并 trim)
 *   - tableDataToCSV: 按 RFC 4180 转义(包含 , " \n 的值用双引号包裹,内部 " 转义为 "")
 *   - tableDataToTSV: tab 分隔,不做转义(TSV 用 \t 分隔本身就罕见特殊字符)
 *   - tableDataToMarkdown: 用 | 分列 --- 做对齐,| / 换行做 escape
 */

export interface TableData {
  headers: string[];
  rows: string[][];
}

export function extractTableData(tableEl: HTMLTableElement): TableData {
  const headers: string[] = [],
    headerCells = tableEl.querySelectorAll("thead th");
  for (const cell of Array.from(headerCells)) {
    headers.push(cell.textContent?.trim() ?? "");
  }

  const rows: string[][] = [],
    bodyRows = tableEl.querySelectorAll("tbody tr");
  for (const row of Array.from(bodyRows)) {
    const rowData: string[] = [],
      cells = row.querySelectorAll("td");
    for (const cell of Array.from(cells)) {
      rowData.push(cell.textContent?.trim() ?? "");
    }
    rows.push(rowData);
  }
  return { headers, rows };
}

export function tableDataToCSV(data: TableData): string {
  const { headers, rows } = data,
    escapeCSV = (value: string): string => {
      let needsEscaping = false,
        hasQuote = false;
      for (const ch of value) {
        if (ch === '"') {
          needsEscaping = true;
          hasQuote = true;
          break;
        }
        if (ch === "," || ch === "\n") {
          needsEscaping = true;
        }
      }
      if (!needsEscaping) {
        return value;
      }
      if (hasQuote) {
        return `"${value.replace(/"/g, '""')}"`;
      }
      return `"${value}"`;
    },
    totalRows = headers.length > 0 ? rows.length + 1 : rows.length,
    csvRows: string[] = new Array(totalRows);
  let rowIndex = 0;
  if (headers.length > 0) {
    csvRows[rowIndex] = headers.map(escapeCSV).join(",");
    rowIndex += 1;
  }
  for (const row of rows) {
    if (row.length < headers.length) {
      const paddedRow: string[] = new Array(headers.length);
      for (let i = 0; i < headers.length; i += 1) {
        paddedRow[i] = i < row.length ? escapeCSV(row[i]) : "";
      }
      csvRows[rowIndex] = paddedRow.join(",");
    } else {
      csvRows[rowIndex] = row.map(escapeCSV).join(",");
    }
    rowIndex += 1;
  }
  return csvRows.join("\n");
}

export function tableDataToTSV(data: TableData): string {
  const { headers, rows } = data,
    totalRows = headers.length > 0 ? rows.length + 1 : rows.length,
    tsvRows: string[] = new Array(totalRows);
  let rowIndex = 0;
  if (headers.length > 0) {
    tsvRows[rowIndex] = headers.join("\t");
    rowIndex += 1;
  }
  for (const row of rows) {
    if (row.length < headers.length) {
      const paddedRow: string[] = new Array(headers.length);
      for (let i = 0; i < headers.length; i += 1) {
        paddedRow[i] = i < row.length ? row[i] : "";
      }
      tsvRows[rowIndex] = paddedRow.join("\t");
    } else {
      tsvRows[rowIndex] = row.join("\t");
    }
    rowIndex += 1;
  }
  return tsvRows.join("\n");
}

export function tableDataToMarkdown(data: TableData): string {
  const { headers, rows } = data;
  if (headers.length === 0) {
    return "";
  }
  const markdownRows: string[] = new Array(rows.length + 2);
  let rowIndex = 0;
  markdownRows[rowIndex] = `| ${headers.map((h) => escapeMarkdownTableCell(h)).join(" | ")} |`;
  rowIndex += 1;
  const separatorParts: string[] = new Array(headers.length);
  for (let i = 0; i < headers.length; i += 1) {
    separatorParts[i] = "---";
  }
  markdownRows[rowIndex] = `| ${separatorParts.join(" | ")} |`;
  rowIndex += 1;
  for (const row of rows) {
    if (row.length < headers.length) {
      const paddedRow: string[] = new Array(headers.length);
      for (let i = 0; i < headers.length; i += 1) {
        paddedRow[i] = i < row.length ? escapeMarkdownTableCell(row[i]) : "";
      }
      markdownRows[rowIndex] = `| ${paddedRow.join(" | ")} |`;
    } else {
      markdownRows[rowIndex] =
        `| ${row.map((cell) => escapeMarkdownTableCell(cell)).join(" | ")} |`;
    }
    rowIndex += 1;
  }
  return markdownRows.join("\n");
}

function escapeMarkdownTableCell(value: string): string {
  // 跟库一致:把 | 与换行转成转义形式,以保证 markdown 表格语法不被破坏
  return value
    .replace(/\\/g, "\\\\")
    .replace(/\|/g, "\\|")
    .replace(/\n/g, "\\n")
    .replace(/\r/g, "\\r");
}
