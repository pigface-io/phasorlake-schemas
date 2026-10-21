/*==============================================================*/
/* Data Model:     AEMO Electricity Data Model v5.6.0.1 postgreSQL*/
/* Created on:     2025/10/09 17:18:34                          */
/*==============================================================*/


/*==============================================================*/
/* Table: DISPATCHPRICE                                         */
/*==============================================================*/
create table DISPATCHPRICE (
   SETTLEMENTDATE       TIMESTAMP(0)         not null,
   RUNNO                NUMERIC(3,0)         not null,
   REGIONID             VARCHAR(10)          not null,
   DISPATCHINTERVAL     VARCHAR(22)          not null,
   INTERVENTION         NUMERIC(2,0)         not null,
   RRP                  NUMERIC(15,5)        null,
   EEP                  NUMERIC(15,5)        null,
   ROP                  NUMERIC(15,5)        null,
   APCFLAG              NUMERIC(3,0)         null,
   MARKETSUSPENDEDFLAG  NUMERIC(3,0)         null,
   LASTCHANGED          TIMESTAMP(0)         null,
   RAISE6SECRRP         NUMERIC(15,5)        null,
   RAISE6SECROP         NUMERIC(15,5)        null,
   RAISE6SECAPCFLAG     NUMERIC(3,0)         null,
   RAISE60SECRRP        NUMERIC(15,5)        null,
   RAISE60SECROP        NUMERIC(15,5)        null,
   RAISE60SECAPCFLAG    NUMERIC(3,0)         null,
   RAISE5MINRRP         NUMERIC(15,5)        null,
   RAISE5MINROP         NUMERIC(15,5)        null,
   RAISE5MINAPCFLAG     NUMERIC(3,0)         null,
   RAISEREGRRP          NUMERIC(15,5)        null,
   RAISEREGROP          NUMERIC(15,5)        null,
   RAISEREGAPCFLAG      NUMERIC(3,0)         null,
   LOWER6SECRRP         NUMERIC(15,5)        null,
   LOWER6SECROP         NUMERIC(15,5)        null,
   LOWER6SECAPCFLAG     NUMERIC(3,0)         null,
   LOWER60SECRRP        NUMERIC(15,5)        null,
   LOWER60SECROP        NUMERIC(15,5)        null,
   LOWER60SECAPCFLAG    NUMERIC(3,0)         null,
   LOWER5MINRRP         NUMERIC(15,5)        null,
   LOWER5MINROP         NUMERIC(15,5)        null,
   LOWER5MINAPCFLAG     NUMERIC(3,0)         null,
   LOWERREGRRP          NUMERIC(15,5)        null,
   LOWERREGROP          NUMERIC(15,5)        null,
   LOWERREGAPCFLAG      NUMERIC(3,0)         null,
   PRICE_STATUS         VARCHAR(20)          null,
   PRE_AP_ENERGY_PRICE  NUMERIC(15,5)        null,
   PRE_AP_RAISE6_PRICE  NUMERIC(15,5)        null,
   PRE_AP_RAISE60_PRICE NUMERIC(15,5)        null,
   PRE_AP_RAISE5MIN_PRICE NUMERIC(15,5)        null,
   PRE_AP_RAISEREG_PRICE NUMERIC(15,5)        null,
   PRE_AP_LOWER6_PRICE  NUMERIC(15,5)        null,
   PRE_AP_LOWER60_PRICE NUMERIC(15,5)        null,
   PRE_AP_LOWER5MIN_PRICE NUMERIC(15,5)        null,
   PRE_AP_LOWERREG_PRICE NUMERIC(15,5)        null,
   CUMUL_PRE_AP_ENERGY_PRICE NUMERIC(15,5)        null,
   CUMUL_PRE_AP_RAISE6_PRICE NUMERIC(15,5)        null,
   CUMUL_PRE_AP_RAISE60_PRICE NUMERIC(15,5)        null,
   CUMUL_PRE_AP_RAISE5MIN_PRICE NUMERIC(15,5)        null,
   CUMUL_PRE_AP_RAISEREG_PRICE NUMERIC(15,5)        null,
   CUMUL_PRE_AP_LOWER6_PRICE NUMERIC(15,5)        null,
   CUMUL_PRE_AP_LOWER60_PRICE NUMERIC(15,5)        null,
   CUMUL_PRE_AP_LOWER5MIN_PRICE NUMERIC(15,5)        null,
   CUMUL_PRE_AP_LOWERREG_PRICE NUMERIC(15,5)        null,
   OCD_STATUS           VARCHAR(14)          null,
   MII_STATUS           VARCHAR(21)          null,
   RAISE1SECRRP         NUMERIC(15,5)        null,
   RAISE1SECROP         NUMERIC(15,5)        null,
   RAISE1SECAPCFLAG     NUMERIC(3,0)         null,
   LOWER1SECRRP         NUMERIC(15,5)        null,
   LOWER1SECROP         NUMERIC(15,5)        null,
   LOWER1SECAPCFLAG     NUMERIC(3,0)         null,
   PRE_AP_RAISE1_PRICE  NUMERIC(15,5)        null,
   PRE_AP_LOWER1_PRICE  NUMERIC(15,5)        null,
   CUMUL_PRE_AP_RAISE1_PRICE NUMERIC(15,5)        null,
   CUMUL_PRE_AP_LOWER1_PRICE NUMERIC(15,5)        null
);

comment on table DISPATCHPRICE is
'DISPATCHPRICE records 5 minute dispatch prices for energy and FCAS, including whether an intervention has occurred, or price override (e.g. for Administered Price Cap). DISPATCHPRICE updates when price adjustments occur, in which case the new price is written to the RRP field, and the old price to the ROP field as an audit trail.';

comment on column DISPATCHPRICE.SETTLEMENTDATE is
'Market date and time starting at 04:05';

comment on column DISPATCHPRICE.RUNNO is
'Dispatch run no; always 1';

comment on column DISPATCHPRICE.REGIONID is
'Region Identifier';

comment on column DISPATCHPRICE.DISPATCHINTERVAL is
'Dispatch interval identifier 001 to 288 in format YYYYMMDDPPP';

comment on column DISPATCHPRICE.INTERVENTION is
'Manual intervention flag';

comment on column DISPATCHPRICE.RRP is
'Regional Reference Price for this dispatch period. RRP is the price used to settle the market';

comment on column DISPATCHPRICE.EEP is
'Excess energy price - no longer used';

comment on column DISPATCHPRICE.ROP is
'Regional Override Price, being the original price prior to any price scaling, price capping or VoLL override being applied. The APC flag allows the determination of whether capping, scaling or override occurred';

comment on column DISPATCHPRICE.APCFLAG is
'APC Active flag (see note)';

comment on column DISPATCHPRICE.MARKETSUSPENDEDFLAG is
'Market suspended flag';

comment on column DISPATCHPRICE.LASTCHANGED is
'Last date and time record changed';

comment on column DISPATCHPRICE.PRICE_STATUS is
'Status of regional prices for this dispatch interval "NOT FIRM" or "FIRM"';

comment on column DISPATCHPRICE.PRE_AP_ENERGY_PRICE is
'Price before ap capping or scaling - for rolling sum price monitoring';

comment on column DISPATCHPRICE.PRE_AP_RAISE6_PRICE is
'Price before ap capping or scaling - for rolling sum price monitoring';

comment on column DISPATCHPRICE.PRE_AP_RAISE60_PRICE is
'Price before ap capping or scaling - for rolling sum price monitoring';

comment on column DISPATCHPRICE.PRE_AP_RAISE5MIN_PRICE is
'Price before ap capping or scaling - for rolling sum price monitoring';

comment on column DISPATCHPRICE.PRE_AP_RAISEREG_PRICE is
'Price before ap capping or scaling - for rolling sum price monitoring';

comment on column DISPATCHPRICE.PRE_AP_LOWER6_PRICE is
'Price before ap capping or scaling - for rolling sum price monitoring';

comment on column DISPATCHPRICE.PRE_AP_LOWER60_PRICE is
'Price before ap capping or scaling - for rolling sum price monitoring';

comment on column DISPATCHPRICE.PRE_AP_LOWER5MIN_PRICE is
'Price before ap capping or scaling - for rolling sum price monitoring';

comment on column DISPATCHPRICE.PRE_AP_LOWERREG_PRICE is
'Price before ap capping or scaling - for rolling sum price monitoring';

comment on column DISPATCHPRICE.CUMUL_PRE_AP_ENERGY_PRICE is
'Cumulative price that triggers administered pricing event if above the threshold';

comment on column DISPATCHPRICE.CUMUL_PRE_AP_RAISE6_PRICE is
'Cumulative price that triggers administered pricing event if above the threshold';

comment on column DISPATCHPRICE.CUMUL_PRE_AP_RAISE60_PRICE is
'Cumulative price that triggers administered pricing event if above the threshold';

comment on column DISPATCHPRICE.CUMUL_PRE_AP_RAISE5MIN_PRICE is
'Cumulative price that triggers administered pricing event if above the threshold';

comment on column DISPATCHPRICE.CUMUL_PRE_AP_RAISEREG_PRICE is
'Cumulative price that triggers administered pricing event if above the threshold';

comment on column DISPATCHPRICE.CUMUL_PRE_AP_LOWER6_PRICE is
'Cumulative price that triggers administered pricing event if above the threshold';

comment on column DISPATCHPRICE.CUMUL_PRE_AP_LOWER60_PRICE is
'Cumulative price that triggers administered pricing event if above the threshold';

comment on column DISPATCHPRICE.CUMUL_PRE_AP_LOWER5MIN_PRICE is
'Cumulative price that triggers administered pricing event if above the threshold';

comment on column DISPATCHPRICE.CUMUL_PRE_AP_LOWERREG_PRICE is
'Cumulative price that triggers administered pricing event if above the threshold';

comment on column DISPATCHPRICE.OCD_STATUS is
'Communicates the current OCD status for this dispatch interval.  Values of: ''NOT_OCD'', ''OCD_UNRESOLVED'', ''OCD_RESOLVED''.';

comment on column DISPATCHPRICE.MII_STATUS is
'Communicates the current MII status for this dispatch interval.  Values of: ''NOT_MII'', ''MII_SUBJECT_TO_REVIEW'', ''MII_PRICE_REJECTED'', ''MII_PRICE_ACCEPTED''.';

comment on column DISPATCHPRICE.RAISE1SECRRP is
'Regional Raise 1Sec Price - R1Price attribute after capping/flooring';

comment on column DISPATCHPRICE.RAISE1SECROP is
'Raise1Sec Regional Original Price - uncapped/unfloored and unscaled';

comment on column DISPATCHPRICE.RAISE1SECAPCFLAG is
'BitFlag field for Price adjustments - "1" = Voll_Override; "4" = Floor_VoLL; "8" = Manual_Override; "16" = Price_Scaled';

comment on column DISPATCHPRICE.LOWER1SECRRP is
'Regional Lower 1Sec Price - RegionSolution element L1Price attribute';

comment on column DISPATCHPRICE.LOWER1SECROP is
'Lower1Sec Regional Original Price - uncapped/unfloored and unscaled';

comment on column DISPATCHPRICE.LOWER1SECAPCFLAG is
'BitFlag field for Price adjustments - "1" = Voll_Override; "4" = Floor_VoLL; "8" = Manual_Override; "16" = Price_Scaled';

comment on column DISPATCHPRICE.PRE_AP_RAISE1_PRICE is
'Price before AP capping or scaling - for Rolling Sum Price monitoring';

comment on column DISPATCHPRICE.PRE_AP_LOWER1_PRICE is
'Price before AP capping or scaling - for Rolling Sum Price monitoring';

comment on column DISPATCHPRICE.CUMUL_PRE_AP_RAISE1_PRICE is
'Cumulative price that triggers administered pricing event if above the threshold';

comment on column DISPATCHPRICE.CUMUL_PRE_AP_LOWER1_PRICE is
'Cumulative price that triggers administered pricing event if above the threshold';

alter table DISPATCHPRICE
   add constraint PK_DISPATCHPRICE primary key (SETTLEMENTDATE, RUNNO, REGIONID, DISPATCHINTERVAL, INTERVENTION);

/*==============================================================*/
/* Index: DISPATCHPRICE_LCX                                     */
/*==============================================================*/
create  index DISPATCHPRICE_LCX on DISPATCHPRICE (
LASTCHANGED
);

/*==============================================================*/
/* Table: REGION                                                */
/*==============================================================*/
create table REGION (
   REGIONID             VARCHAR(10)          not null,
   DESCRIPTION          VARCHAR(64)          null,
   REGIONSTATUS         VARCHAR(8)           null,
   LASTCHANGED          TIMESTAMP(0)         null
);

comment on table REGION is
'REGION sets out valid region IDs.';

comment on column REGION.REGIONID is
'Differentiates this region from all other regions';

comment on column REGION.DESCRIPTION is
'Full description of region';

comment on column REGION.REGIONSTATUS is
'Status of the region e.g. working, inactive, archive.';

comment on column REGION.LASTCHANGED is
'Last date and time record changed';

alter table REGION
   add constraint REGION_PK primary key (REGIONID);

/*==============================================================*/
/* Index: REGION_LCX                                            */
/*==============================================================*/
create  index REGION_LCX on REGION (
LASTCHANGED
);
