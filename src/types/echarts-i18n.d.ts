/**
 * echarts 语言包只发布 UMD JS,无类型声明;按 registerLocale 的 localeObj 形参补声明。
 * (echarts/types 下虽有 src/i18n/*.d.ts,但 exports 映射的 i18n/langZH-obj.js 不带类型)
 */
declare module "echarts/i18n/langZH-obj.js" {
  const lang: Parameters<typeof import("echarts/core").registerLocale>[1];
  export default lang;
}
