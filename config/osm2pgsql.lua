-- Flex-mode config: import ONLY POIs and places.
--
-- A full OSM import of Indonesia is 20GB+ and the agent uses none of it. Filtering
-- to the tag classes below keeps the database around 4GB and every bbox query fast.
-- Targeted by the `search_poi` tool (osm_poi) and used for coarse place lookup
-- (osm_place); fuzzy name geocoding is Photon's job, not this table's.

local tables = {}

local common_columns = {
  { column = 'name', type = 'text' },
  { column = 'tags', type = 'jsonb' },
  { column = 'geom', type = 'point', projection = 4326, not_null = true },
}

tables.poi = osm2pgsql.define_table{
  name = 'osm_poi',
  ids = { type = 'any', id_column = 'osm_id', type_column = 'osm_type' },
  columns = common_columns,
}

tables.place = osm2pgsql.define_table{
  name = 'osm_place',
  ids = { type = 'any', id_column = 'osm_id', type_column = 'osm_type' },
  columns = common_columns,
}

-- Retained tag classes. Extend deliberately: every key added here grows the table
-- and the agent can only ask for tags that were imported.
local poi_keys = { 'natural', 'tourism', 'amenity', 'historic', 'leisure' }

local function is_poi(tags)
  for _, k in ipairs(poi_keys) do
    if tags[k] then return true end
  end
  return false
end

local function add(tbl, object, geom)
  if geom == nil or geom:is_null() then return end
  tbl:insert{ name = object.tags.name, tags = object.tags, geom = geom }
end

function osm2pgsql.process_node(object)
  if next(object.tags) == nil then return end
  if object.tags.place then add(tables.place, object, object:as_point()) end
  if is_poi(object.tags) then add(tables.poi, object, object:as_point()) end
end

function osm2pgsql.process_way(object)
  if not object.is_closed then return end
  if next(object.tags) == nil then return end
  -- Areas are reduced to a representative point: the agent needs "where is it",
  -- not the footprint, and a point keeps the GiST index small.
  if is_poi(object.tags) then
    add(tables.poi, object, object:as_polygon():centroid())
  end
end

function osm2pgsql.process_relation(object)
  if object.tags.type ~= 'multipolygon' then return end
  if is_poi(object.tags) then
    add(tables.poi, object, object:as_multipolygon():centroid())
  end
end
