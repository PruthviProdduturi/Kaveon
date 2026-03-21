"use client";

/**
 * Shared brand SVG icons for all supported data source / metadata DB types.
 * Import from here to ensure consistency across data-sources page, SetupModal, and metadata settings.
 */

import React from "react";

// ── Brand icons ───────────────────────────────────────────────────────────────

export const FabricIcon = () => (
  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="20" height="20">
    <defs>
      <linearGradient id="fi-a" x1="6.477" x2="6.477" y1="22.003" y2="14.73" gradientUnits="userSpaceOnUse">
        <stop offset=".056" stopColor="#2AAC94"/><stop offset=".155" stopColor="#239C87"/>
        <stop offset=".372" stopColor="#177E71"/><stop offset=".588" stopColor="#0E6961"/>
        <stop offset=".799" stopColor="#095D57"/><stop offset="1" stopColor="#085954"/>
      </linearGradient>
      <linearGradient id="fi-b" x1="15.667" x2="8.644" y1="16.726" y2="9.087" gradientUnits="userSpaceOnUse">
        <stop offset=".042" stopColor="#ABE88E"/><stop offset=".549" stopColor="#2AAA92"/>
        <stop offset=".906" stopColor="#117865"/>
      </linearGradient>
      <linearGradient id="fi-c" x1="-1.592" x2="5.092" y1="16.354" y2="14.075" gradientUnits="userSpaceOnUse">
        <stop stopColor="#6AD6F9"/><stop offset="1" stopColor="#6AD6F9" stopOpacity="0"/>
      </linearGradient>
      <linearGradient id="fi-d" x1="3.507" x2="21.297" y1="7.61" y2="7.61" gradientUnits="userSpaceOnUse">
        <stop offset=".043" stopColor="#25FFD4"/><stop offset=".874" stopColor="#55DDB9"/>
      </linearGradient>
      <linearGradient id="fi-e" x1="3.507" x2="19.532" y1="5.124" y2="12.565" gradientUnits="userSpaceOnUse">
        <stop stopColor="#6AD6F9"/><stop offset=".23" stopColor="#60E9D0"/>
        <stop offset=".651" stopColor="#6DE9BB"/><stop offset=".994" stopColor="#ABE88E"/>
      </linearGradient>
      <linearGradient id="fi-f" x1="4.989" x2="13.703" y1="6.516" y2="8.444" gradientUnits="userSpaceOnUse">
        <stop stopColor="#fff" stopOpacity="0"/><stop offset=".459" stopColor="#fff"/>
        <stop offset="1" stopColor="#fff" stopOpacity="0"/>
      </linearGradient>
      <linearGradient id="fi-g" x1="7.879" x2="8.085" y1="13.981" y2="7.87" gradientUnits="userSpaceOnUse">
        <stop offset=".205" stopColor="#063D3B" stopOpacity="0"/><stop offset=".586" stopColor="#063D3B" stopOpacity=".237"/>
        <stop offset=".872" stopColor="#063D3B" stopOpacity=".75"/>
      </linearGradient>
      <linearGradient id="fi-h" x1="1.405" x2="8.851" y1="13.373" y2="14.774" gradientUnits="userSpaceOnUse">
        <stop stopColor="#fff" stopOpacity="0"/><stop offset=".459" stopColor="#fff"/>
        <stop offset="1" stopColor="#fff" stopOpacity="0"/>
      </linearGradient>
      <linearGradient id="fi-i" x1="6.784" x2="5.331" y1="19.988" y2="12.884" gradientUnits="userSpaceOnUse">
        <stop offset=".064" stopColor="#063D3B" stopOpacity="0"/><stop offset=".17" stopColor="#063D3B" stopOpacity=".135"/>
        <stop offset=".562" stopColor="#063D3B" stopOpacity=".599"/><stop offset=".85" stopColor="#063D3B" stopOpacity=".9"/>
        <stop offset="1" stopColor="#063D3B"/>
      </linearGradient>
    </defs>
    <path fill="url(#fi-a)" fillRule="evenodd" d="m2.82 15.802-.293 1.072c-.11.343-.262.847-.345 1.295a2.815 2.815 0 0 0 2.32 3.795c.396.057.844.054 1.346-.02l2.307-.318a1.46 1.46 0 0 0 1.21-1.064l1.588-5.832z" clipRule="evenodd"/>
    <path fill="url(#fi-b)" d="M5.07 16.078c-2.431.376-2.93 2.211-2.93 2.211l2.328-8.556 12.168-1.646-1.66 6.027a.85.85 0 0 1-.693.622l-.068.011-9.213 1.342z"/>
    <path fill="url(#fi-c)" fillOpacity=".8" d="M5.07 16.078c-2.431.376-2.93 2.211-2.93 2.211l2.328-8.556 12.168-1.646-1.66 6.027a.85.85 0 0 1-.693.622l-.068.011-9.213 1.342z"/>
    <path fill="url(#fi-d)" d="m6.45 10.619 13.47-1.99a.8.8 0 0 0 .662-.586l1.39-5.03a.797.797 0 0 0-.87-1.006L8.25 3.905a3.59 3.59 0 0 0-2.89 2.597L3.507 13.22c.372-1.36.6-2.178 2.943-2.602Z"/>
    <path fill="url(#fi-e)" d="m6.45 10.619 13.47-1.99a.8.8 0 0 0 .662-.586l1.39-5.03a.797.797 0 0 0-.87-1.006L8.25 3.905a3.59 3.59 0 0 0-2.89 2.597L3.507 13.22c.372-1.36.6-2.178 2.943-2.602Z"/>
    <path fill="url(#fi-f)" fillOpacity=".4" d="m6.45 10.619 13.47-1.99a.8.8 0 0 0 .662-.586l1.39-5.03a.797.797 0 0 0-.87-1.006L8.25 3.905a3.59 3.59 0 0 0-2.89 2.597L3.507 13.22c.372-1.36.6-2.178 2.943-2.602Z"/>
    <path fill="url(#fi-g)" d="M6.45 10.619c-1.95.353-2.435.981-2.757 1.966L2.139 18.29s.497-1.816 2.899-2.205l9.177-1.337.068-.01a.85.85 0 0 0 .694-.623l1.365-4.958z"/>
    <path fill="url(#fi-h)" fillOpacity=".2" d="M6.45 10.619c-1.95.353-2.435.981-2.757 1.966L2.139 18.29s.497-1.816 2.899-2.205l9.177-1.337.068-.01a.85.85 0 0 0 .694-.623l1.365-4.958z"/>
    <path fill="url(#fi-i)" fillRule="evenodd" d="M5.038 16.086c-2.03.328-2.697 1.673-2.856 2.082a2.817 2.817 0 0 0 2.32 3.796c.396.057.844.054 1.346-.02l2.307-.318a1.46 1.46 0 0 0 1.21-1.064l1.448-5.317z" clipRule="evenodd"/>
  </svg>
);

export const AzureSqlIcon = () => (
  <svg viewBox="0 0 24 24" width="20" height="20" fill="#0078D4">
    <path d="M10.432.006L5.29 13.53l4.261 7.67h13.957L17.636 11.09 10.432.006zM5.054 15.233L.492 21.2H6.95l-1.896-5.967z"/>
  </svg>
);

export const PostgreSQLIcon = () => (
  <svg viewBox="0 0 24 24" width="20" height="20">
    <path fill="#336791" d="M23.5594 14.7228a.5269.5269 0 0 0-.0563-.1191c-.139-.2632-.4768-.3418-1.0074-.2321-1.6533.3411-2.2935.1312-2.5256-.0191 1.342-2.0482 2.445-4.522 3.0411-6.8297.2714-1.0507.7982-3.5237.1222-4.7316a1.5641 1.5641 0 0 0-.1509-.235C21.6931.9086 19.8007.0248 17.5099.0005c-1.4947-.0158-2.7705.3461-3.1161.4794a9.449 9.449 0 0 0-.5159-.0816 8.044 8.044 0 0 0-1.3114-.1278c-1.1822-.0184-2.2038.2642-3.0498.8406-.8573-.3211-4.7888-1.645-7.2219.0788C.9359 2.1526.3086 3.8733.4302 6.3043c.0409.818.5069 3.334 1.2423 5.7436.4598 1.5065.9387 2.7019 1.4334 3.582.553.9942 1.1259 1.5933 1.7143 1.7895.4474.1491 1.1327.1441 1.8581-.7279.8012-.9635 1.5903-1.8258 1.9446-2.2069.4351.2355.9064.3625 1.39.3772a.0569.0569 0 0 0 .0004.0041 11.0312 11.0312 0 0 0-.2472.3054c-.3389.4302-.4094.5197-1.5002.7443-.3102.064-1.1344.2339-1.1464.8115-.0025.1224.0329.2309.0919.3268.2269.4231.9216.6097 1.015.6331 1.3345.3335 2.5044.092 3.3714-.6787-.017 2.231.0775 4.4174.3454 5.0874.2212.5529.7618 1.9045 2.4692 1.9043.2505 0 .5263-.0291.8296-.0941 1.7819-.3821 2.5557-1.1696 2.855-2.9059.1503-.8707.4016-2.8753.5388-4.1012.0169-.0703.0357-.1207.057-.1362.0007-.0005.0697-.0471.4272.0307a.3673.3673 0 0 0 .0443.0068l.2539.0223.0149.001c.8468.0384 1.9114-.1426 2.5312-.4308.6438-.2988 1.8057-1.0323 1.5951-1.6698z"/>
  </svg>
);

export const MySQLIcon = () => (
  <svg viewBox="0 0 24 24" width="20" height="20">
    <path fill="#4479A1" d="M16.405 5.501c-.115 0-.193.014-.274.033v.013h.014c.054.104.146.18.214.273.054.107.1.214.154.32l.014-.015c.094-.066.14-.172.14-.333-.04-.047-.046-.094-.08-.134-.04-.064-.14-.108-.18-.157zM12 0C5.373 0 0 5.373 0 12s5.373 12 12 12 12-5.373 12-12S18.627 0 12 0zm5.187 17.966c-.36.072-.744.072-1.08 0-.293-.065-.53-.228-.724-.47-.194-.242-.307-.554-.307-.915 0-.377.12-.68.328-.913.216-.227.49-.358.807-.358.3 0 .548.104.74.307.193.2.307.484.307.84h-1.604c.01.264.105.468.26.596.147.124.35.19.59.19.22 0 .41-.046.565-.137.155-.09.27-.217.336-.375l.38.15c-.09.23-.25.42-.465.55zm-8.5.234L7.5 14.2l-1.187 4h-.9L4 13.2h.92l.987 3.7 1.19-3.7h.81l1.19 3.7.987-3.7h.92l-1.413 5zm4.04 0h-.875v-5h.875v5zm3.14-4.15c-.25 0-.44.09-.57.27s-.19.43-.19.75v3.13h-.875v-5h.84v.75c.11-.26.27-.46.48-.6.22-.15.47-.22.77-.22l.09.01v.87c-.18-.02-.36-.01-.545.04z"/>
  </svg>
);

export const TrinoIcon = () => (
  <svg viewBox="0 0 24 24" width="20" height="20" fill="#DD1C1C">
    <path d="M14.124 16.8529a.1615.1615 0 1 1 .1576.1614.1577.1577 0 0 1-.1576-.1614zm-5.607-.1576a.1614.1614 0 1 0 0 .3228.1614.1614 0 0 0 0-.3228zm10.1341-.6648v1.9869c-.031.5788-.524 1.0237-1.1029.9954h-.3843a5.0596 5.0596 0 0 1-1.1298 1.7178.3192.3192 0 0 0 0 .465l.2382.2191a.3036.3036 0 0 1 .0385.4304c-1.126 1.3835-2.9669 2.1521-5.0498 2.1521a6.575 6.575 0 0 1-4.8192-1.8985c-.0029-.0032-.0059-.0063-.0087-.0096a.6302.6302 0 0 1 .0548-.8896c.137-.1265.1371-.3462 0-.4727a4.944 4.944 0 0 1-1.126-1.714h-.3497c-.5797.0284-1.0737-.416-1.1068-.9954v-1.9869c.0351-.5779.5286-1.02 1.1068-.9915h.2728a5.7648 5.7648 0 0 1 2.0791-3.0936c-.4227-1.0991-1.1529-3.2551-1.226-5.0075C6.0229 4.4705 6.2189.078 7.8253.001c1.6064-.0768 1.3719 4.0275 1.0991 6.6946a32.732 32.732 0 0 0-.123 4.4503 6.994 6.994 0 0 1 2.4826-.4304 7.2414 7.2414 0 0 1 1.7371.2075c.2614-1.2682.8762-3.574 2.0292-5.1958c1.6717-2.352 3.4357-4.7808 4.6116-4.1006c1.176.6802-.3074 3.1398-1.3297 4.4272c-1.0222 1.2874-2.7862 3.2089-3.3742 4.2274c-.2114.3843-.4304.8032-.5956 1.1529a5.7375 5.7375 0 0 1 2.9169 3.6125h.073v-2.3058a.3075.3075 0 0 0-.1806-.2844a.9148.9148 0 0 1-.5573-.8148a1.0184 1.0184 0 0 1 .9045-.9044c.5593-.0598 1.061.3452 1.1208.9044a.9187.9187 0 0 1-.5534.8148a.3074.3074 0 0 0-.1691.2844v2.1522a.3113.3113 0 0 0 .1691.2805a.9724.9724 0 0 1 .5648.857z"/>
  </svg>
);

export const StarRocksIcon = () => (
  <svg viewBox="0 0 100 100" width="20" height="20">
    <path fill="#01808F" d="M11.8,26.4c-0.1,2.2,0.9,3.4,2.3,4.5c9,7.4,18,14.8,27,22.2c2.5,2.1,2.7,3.5,1,6.2c-4.4,6.7-8.8,13.4-13.2,20.1c-1.7,2.5-3.2,2.9-5.9,1.4c-2.8-1.6-5.6-3.2-8.4-4.8c-3.2-1.8-4.8-4.6-4.8-8.3c0-11.8,0-23.6,0-35.5C9.8,30.2,10.3,28.4,11.8,26.4z"/>
    <path fill="#01808F" d="M87.9,73.8c0.6-2.3-0.5-3.5-1.9-4.6c-9-7.4-17.9-14.7-26.8-22.1c-2.9-2.4-3.1-3.6-1.1-6.7c4.3-6.6,8.7-13.2,13-19.8c1.7-2.6,3.3-2.9,6-1.4c2.8,1.6,5.6,3.2,8.3,4.8c3.2,1.8,4.7,4.5,4.7,8.2c0,11.9,0,23.7,0,35.6C90.2,69.9,89.6,71.9,87.9,73.8z"/>
    <path fill="#01808F" d="M67.1,56.8c0.6,0.4,17.3,14.1,17.5,14.3c2.4,2.2,2.2,4.1-0.6,5.8C76.8,81,57.3,92.1,54.8,93.6c-3.2,1.9-6.4,1.9-9.6,0c-4.6-2.7-9.2-5.3-13.8-8c-2.2-1.3-2.2-2.7,0-3.9C42.3,75.5,53.1,69.2,64,63C66.5,61.6,68.2,60.1,67.1,56.8z"/>
    <path fill="#FEBD02" d="M32.9,43.2c-0.6-0.4-17.3-14.1-17.5-14.3c-2.4-2.2-2.2-4.1,0.6-5.8C23.2,19,42.7,7.9,45.2,6.4c3.2-1.9,6.4-1.9,9.6,0c4.6,2.7,9.2,5.3,13.8,8c2.2,1.3,2.2,2.7,0,3.9C57.7,24.5,46.9,30.8,36,37C33.5,38.4,31.8,39.9,32.9,43.2z"/>
  </svg>
);

// ── Metadata map used by SetupModal (keyed by DbType: fabric_sql etc.) ────────

export interface DbTypeMeta {
  icon: React.ReactNode;
  color: string;
  bg: string;
  border: string;
}

export const SETUP_DB_ICONS: Record<string, DbTypeMeta> = {
  fabric_sql: { icon: <FabricIcon />, color: "#0E6961", bg: "#f0fdf9", border: "#99f6e4" },
  azure_sql:  { icon: <AzureSqlIcon />, color: "#0078D4", bg: "#eff6ff", border: "#93c5fd" },
  postgresql: { icon: <PostgreSQLIcon />, color: "#336791", bg: "#f0f7ff", border: "#93c5fd" },
  mysql:      { icon: <MySQLIcon />, color: "#4479A1", bg: "#eff6ff", border: "#93c5fd" },
};

// ── Data source page map (keyed by display type: "Fabric SQL AEP" etc.) ───────

export interface SourceTypeMeta {
  label: string;
  icon: React.ReactNode;
  color: string;
  bg: string;
  border: string;
  endpointLabel: string;
  endpointPlaceholder: string;
}

export const SOURCE_TYPE_META: Record<string, SourceTypeMeta> = {
  "Fabric SQL AEP": {
    label: "Fabric SQL AEP", icon: <FabricIcon />,
    color: "#0E6961", bg: "#f0fdf9", border: "#99f6e4",
    endpointLabel: "SQL Endpoint",
    endpointPlaceholder: "your-workspace.datawarehouse.fabric.microsoft.com",
  },
  "Fabric SQL DW": {
    label: "Fabric SQL DW", icon: <FabricIcon />,
    color: "#0E6961", bg: "#f0fdf9", border: "#99f6e4",
    endpointLabel: "SQL Endpoint",
    endpointPlaceholder: "your-workspace.datawarehouse.fabric.microsoft.com",
  },
  "Azure SQL": {
    label: "Azure SQL", icon: <AzureSqlIcon />,
    color: "#0078D4", bg: "#eff6ff", border: "#93c5fd",
    endpointLabel: "Server Endpoint",
    endpointPlaceholder: "your-server.database.windows.net",
  },
  "PostgreSQL": {
    label: "PostgreSQL", icon: <PostgreSQLIcon />,
    color: "#336791", bg: "#f0f7ff", border: "#93c5fd",
    endpointLabel: "Host:Port",
    endpointPlaceholder: "your-postgres-host:5432",
  },
  "Trino": {
    label: "Trino", icon: <TrinoIcon />,
    color: "#DD1C1C", bg: "#fff1f1", border: "#fca5a5",
    endpointLabel: "Coordinator URL",
    endpointPlaceholder: "http://your-coordinator:8080",
  },
  "StarRocks": {
    label: "StarRocks", icon: <StarRocksIcon />,
    color: "#01808F", bg: "#f0fdfa", border: "#99f6e4",
    endpointLabel: "FE Host:Port",
    endpointPlaceholder: "your-starrocks-fe:9030",
  },
};

export const SOURCE_TYPES = Object.keys(SOURCE_TYPE_META);
