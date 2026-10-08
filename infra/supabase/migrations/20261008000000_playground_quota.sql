-- Playground free-tier counters (docs/SERVER.md, "Playground API").
--
-- Supabase holds only sign-in (auth.users) and two counters: model-pages per user per UTC day and
-- the free tier's USD spend per UTC day. No documents, outputs or provider keys are stored here.
--
-- The tables live in a private schema that PostgREST does not expose. The gateway reaches them
-- only through the four public.playground_* functions below, with the service-role key; anon and
-- authenticated (that is, the browser) can execute none of them and can read nothing.
--
-- Apply with `supabase db push` (or paste into the SQL editor). Idempotent.

create schema if not exists playground;
revoke all on schema playground from public, anon, authenticated;

create table if not exists playground.user_day (
  user_id     uuid not null references auth.users (id) on delete cascade,
  day         date not null,
  model_pages integer not null default 0 check (model_pages >= 0),
  primary key (user_id, day)
);

create table if not exists playground.budget_day (
  day       date primary key,
  spent_usd numeric(12, 6) not null default 0 check (spent_usd >= 0)
);

alter table playground.user_day enable row level security;
alter table playground.budget_day enable row level security;
-- No policies: with RLS on and no policy, only the table owner and service_role see rows.
revoke all on all tables in schema playground from public, anon, authenticated;

-- Reserve `pages` model-pages for a user today, all-or-nothing, unless that would pass `day_limit`
-- or today's free spend has reached `budget_usd`. One statement, so concurrent runs cannot both
-- squeeze under the limit. Returns the outcome and what the user has left.
create or replace function public.playground_reserve(
  p_user uuid, p_pages integer, p_day_limit integer, p_budget_usd numeric
) returns table (ok boolean, reason text, remaining integer)
language plpgsql
security definer
set search_path = ''
as $$
declare
  today date := (now() at time zone 'utc')::date;
  used integer;
  spent numeric;
begin
  if p_pages <= 0 or p_day_limit < 0 then
    raise exception 'invalid arguments';
  end if;
  select b.spent_usd into spent from playground.budget_day b where b.day = today;
  if p_budget_usd <= 0 or coalesce(spent, 0) >= p_budget_usd then
    select u.model_pages into used from playground.user_day u where u.user_id = p_user and u.day = today;
    return query select false, case when p_budget_usd <= 0 then 'paused' else 'budget_exhausted' end,
      greatest(p_day_limit - coalesce(used, 0), 0);
    return;
  end if;
  if p_pages <= p_day_limit then
    insert into playground.user_day as u (user_id, day, model_pages)
    values (p_user, today, p_pages)
    on conflict (user_id, day) do update
      set model_pages = u.model_pages + excluded.model_pages
      where u.model_pages + excluded.model_pages <= p_day_limit
    returning u.model_pages into used;
  end if;
  if used is null then
    -- Over the limit: the conflicting row was left as it was (or nothing was inserted).
    select u.model_pages into used from playground.user_day u where u.user_id = p_user and u.day = today;
    return query select false, 'quota_exceeded', greatest(p_day_limit - coalesce(used, 0), 0);
    return;
  end if;
  return query select true, null::text, p_day_limit - used;
end;
$$;

-- Give back model-pages reserved for jobs whose submit failed (never below zero).
create or replace function public.playground_release(p_user uuid, p_pages integer)
returns void
language sql
security definer
set search_path = ''
as $$
  update playground.user_day u
     set model_pages = greatest(u.model_pages - greatest(p_pages, 0), 0)
   where u.user_id = p_user and u.day = (now() at time zone 'utc')::date;
$$;

-- Add a succeeded free-tier job's cost to today's spend.
create or replace function public.playground_charge(p_usd numeric)
returns numeric
language sql
security definer
set search_path = ''
as $$
  insert into playground.budget_day as b (day, spent_usd)
  values ((now() at time zone 'utc')::date, greatest(p_usd, 0))
  on conflict (day) do update set spent_usd = b.spent_usd + excluded.spent_usd
  returning b.spent_usd;
$$;

-- What GET /v1/playground/config reports: the user's model-pages left today and today's spend.
create or replace function public.playground_status(p_user uuid, p_day_limit integer)
returns table (remaining integer, spent_usd numeric)
language sql
stable
security definer
set search_path = ''
as $$
  select greatest(p_day_limit - coalesce(
           (select u.model_pages from playground.user_day u
             where u.user_id = p_user and u.day = (now() at time zone 'utc')::date), 0), 0),
         coalesce((select b.spent_usd from playground.budget_day b
                    where b.day = (now() at time zone 'utc')::date), 0);
$$;

revoke all on function public.playground_reserve(uuid, integer, integer, numeric) from public, anon, authenticated;
revoke all on function public.playground_release(uuid, integer) from public, anon, authenticated;
revoke all on function public.playground_charge(numeric) from public, anon, authenticated;
revoke all on function public.playground_status(uuid, integer) from public, anon, authenticated;
grant execute on function public.playground_reserve(uuid, integer, integer, numeric) to service_role;
grant execute on function public.playground_release(uuid, integer) to service_role;
grant execute on function public.playground_charge(numeric) to service_role;
grant execute on function public.playground_status(uuid, integer) to service_role;

-- Counters older than 30 days have no use; drop them with pg_cron if it is enabled:
--   select cron.schedule('playground-prune', '17 3 * * *',
--     $$delete from playground.user_day where day < current_date - 30;
--       delete from playground.budget_day where day < current_date - 30$$);
