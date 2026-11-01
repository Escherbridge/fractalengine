# Deterministic scene setup and strict readback; see tools/qa/AGENTS.md.

function Invoke-QaFixtureApi {
    param([string]$Method, [string]$Path, [string]$Token, $Body)
    $json = if ($null -ne $Body) { $Body | ConvertTo-Json -Depth 20 -Compress } else { $null }
    $response = Invoke-QaApi -Method $Method -Path $Path -Token $Token -JsonBody $json
    if ($response.ok -ne $true -or $null -eq $response.data) {
        throw "fixture: $Method $Path failed: $($response.error)"
    }
    return $response.data
}

function Assert-QaFixtureId {
    param($Value, [string]$Context)
    if ([string]$Value -cnotmatch '^[0-9A-HJKMNP-TV-Z]{26}$') { throw "fixture: $Context returned no valid id" }
    return [string]$Value
}

function Assert-QaFixtureJson {
    param($Actual, $Expected, [string]$Context)
    if ($null -eq $Expected) {
        if ($null -ne $Actual) { throw "fixture: $Context expected null" }
    } elseif ($Expected -is [System.Array]) {
        if ($Actual -isnot [System.Array] -or $Actual.Count -ne $Expected.Count) { throw "fixture: $Context array differs" }
        for ($index = 0; $index -lt $Expected.Count; $index++) {
            Assert-QaFixtureJson $Actual[$index] $Expected[$index] "$Context/$index"
        }
    } elseif ($Expected -is [System.Management.Automation.PSCustomObject]) {
        if ($null -eq $Actual -or @($Actual.PSObject.Properties).Count -ne @($Expected.PSObject.Properties).Count) {
            throw "fixture: $Context object keys differ"
        }
        foreach ($property in $Expected.PSObject.Properties) {
            if ($property.Name -cnotin @($Actual.PSObject.Properties.Name)) { throw "fixture: $Context missing $($property.Name)" }
            Assert-QaFixtureJson $Actual.($property.Name) $property.Value "$Context/$($property.Name)"
        }
    } elseif ($Expected -is [bool]) {
        if ($Actual -isnot [bool] -or $Actual -ne $Expected) { throw "fixture: $Context boolean differs" }
    } elseif ($Expected -is [string]) {
        if ($Actual -isnot [string] -or $Actual -cne $Expected) { throw "fixture: $Context string differs" }
    } else {
        if ($null -eq $Actual -or $Actual -is [string] -or $Actual -is [bool] -or [double]$Actual -ne [double]$Expected) {
            throw "fixture: $Context number differs"
        }
    }
}

function Set-QaFixtureProperties {
    param([string]$NodeId, [hashtable]$Properties, [string]$Token)
    foreach ($key in $Properties.Keys) {
        $write = Invoke-QaFixtureApi PATCH "/api/v1/nodes/$NodeId/properties" $Token @{key=$key; value=$Properties[$key]}
        if ($write.node_id -cne $NodeId -or $write.key -cne $key) { throw 'fixture: property write correlation differs' }
    }
    $read = Invoke-QaFixtureApi GET "/api/v1/nodes/$NodeId/properties" $Token
    if ($read.node_id -cne $NodeId) { throw 'fixture: property read returned another node' }
    foreach ($key in $Properties.Keys) {
        Assert-QaFixtureJson $read.properties.$key $Properties[$key] "node/$NodeId/$key"
    }
}

function New-QaSceneFixture {
    param([ValidateSet('basic','terrain')][string]$Name, [string]$BootstrapToken,
          [string]$TokensPath, [string]$Run, $TerrainDescriptor)
    $verseName = if ($Name -eq 'terrain') { 'QA Terrain' } else { 'QA Basic' }
    $verse = Invoke-QaFixtureApi POST '/api/v1/verses' $BootstrapToken @{name=$verseName}
    $verseId = Assert-QaFixtureId $verse.id 'create verse'
    $owner = Wait-QaOwnerToken -TokensPath $TokensPath -VerseId $verseId -TimeoutSec 30
    if (-not $owner) { throw "fixture: $verseName never got its owner token" }
    $token = $owner.Token
    $fractal = Invoke-QaFixtureApi POST "/api/v1/verses/$verseId/fractals" $token @{name='QA Fractal'}
    $fractalId = Assert-QaFixtureId $fractal.id 'create fractal'
    $petals = @()
    foreach ($petalName in @('Petal A','Petal B')) {
        $petal = Invoke-QaFixtureApi POST "/api/v1/verses/$verseId/fractals/$fractalId/petals" $token @{name=$petalName}
        $petals += Assert-QaFixtureId $petal.id "create $petalName"
    }
    $assetPath = Join-Path $Run 'fractalengine/assets/samples/info_panel.glb'
    $uploaded = Invoke-QaApiUpload -Path "/api/v1/petals/$($petals[0])/assets" -Token $token -FilePath $assetPath
    if ($uploaded.ok -ne $true) { throw "fixture: GLB upload failed: $($uploaded.error)" }
    $assetId = Assert-QaFixtureId $uploaded.data.asset_id 'upload asset'
    $nodeIds = @()
    $positions = @(@(-12,0,0), @(0,0,-12), @(12,0,0))
    for ($index = 0; $index -lt 3; $index++) {
        $node = Invoke-QaFixtureApi POST "/api/v1/verses/$verseId/fractals/$fractalId/petals/$($petals[0])/nodes" $token @{
            name="Cube $($index+1)"; asset_id=$assetId; position=$positions[$index]
        }
        $nodeId = Assert-QaFixtureId $node.id 'create value node'
        $nodeIds += $nodeId
        Set-QaFixtureProperties $nodeId @{qa_fixture='terrain'; qa_role='value'; qa_value=10*($index+1)} $token
    }
    $markerIds = @()
    foreach ($petalId in $petals) {
        $marker = Invoke-QaFixtureApi POST "/api/v1/verses/$verseId/fractals/$fractalId/petals/$petalId/nodes" $token @{
            name='QA oracle origin'; position=@(0,0,0)
        }
        $markerId = Assert-QaFixtureId $marker.id 'create origin marker'
        $markerIds += $markerId
        Set-QaFixtureProperties $markerId @{qa_fixture='terrain'; qa_role='marker'; qa_marker='flat-origin'; qa_base_m=0} $token
    }
    $config = $TerrainDescriptor.terrain_config
    $boundIds = @(if ($Name -eq 'terrain') { $petals } else { $petals[1] })
    foreach ($petalId in $boundIds) {
        $echo = Invoke-QaFixtureApi PUT "/api/v1/petals/$petalId/terrain" $token $config
        $read = Invoke-QaFixtureApi GET "/api/v1/petals/$petalId/terrain" $token
        if ($echo.petal_id -cne $petalId -or $read.petal_id -cne $petalId) { throw 'fixture: terrain returned another petal' }
        Assert-QaFixtureJson $echo.terrain $config "terrain/$petalId/echo"
        Assert-QaFixtureJson $read.terrain $config "terrain/$petalId/readback"
    }
    if ($Name -eq 'basic') {
        $plain = Invoke-QaFixtureApi GET "/api/v1/petals/$($petals[0])/terrain" $token
        if ($plain.petal_id -cne $petals[0]) { throw 'fixture: plain terrain read returned another petal' }
        Assert-QaFixtureJson $plain.terrain $null 'basic/Petal A/terrain'
    }
    $predicate = "petal_id = '$($petals[0])' AND properties.qa_fixture = 'terrain' AND properties.qa_role = 'value' AND tombstone = NONE"
    $rowsSql = "SELECT node_id, petal_id, display_name, position, properties.qa_value AS qa_value FROM node WHERE $predicate ORDER BY qa_value ASC"
    $sumSql = "SELECT count() AS fixture_count, math::sum(properties.qa_value) AS fixture_sum FROM node WHERE $predicate GROUP ALL"
    $rows = Invoke-QaFixtureApi POST '/api/v1/query' $token @{sql=$rowsSql}
    $sum = Invoke-QaFixtureApi POST '/api/v1/query' $token @{sql=$sumSql}
    $queryRows = @($rows.data)
    $sumRows = @($sum.data)
    if ($queryRows.Count -ne 3 -or $sumRows.Count -ne 1) {
        throw 'fixture: scoped query oracle must contain exactly 3 values summing to 60'
    }
    Assert-QaFixtureJson $sumRows[0].fixture_count 3 'query/count'
    Assert-QaFixtureJson $sumRows[0].fixture_sum 60 'query/sum'
    for ($index = 0; $index -lt 3; $index++) {
        if ($queryRows[$index].node_id -cne $nodeIds[$index] -or $queryRows[$index].petal_id -cne $petals[0] -or
            $queryRows[$index].display_name -cne "Cube $($index+1)" -or $queryRows[$index].qa_value -ne 10*($index+1)) {
            throw 'fixture: query row identity or value differs'
        }
        Assert-QaFixtureJson $queryRows[$index].position $positions[$index] "value/$index/position"
    }
    $markers = Invoke-QaFixtureApi POST '/api/v1/query' $token @{
        sql="SELECT node_id, petal_id, position FROM node WHERE properties.qa_fixture = 'terrain' AND properties.qa_role = 'marker' AND tombstone = NONE"
    }
    if (@($markers.data).Count -ne 2) { throw 'fixture: origin marker rows missing' }
    for ($index = 0; $index -lt 2; $index++) {
        $markerRows = @($markers.data | Where-Object { $_.node_id -ceq $markerIds[$index] -and $_.petal_id -ceq $petals[$index] })
        if ($markerRows.Count -ne 1) { throw 'fixture: origin marker hierarchy differs' }
        Assert-QaFixtureJson $markerRows[0].position @(0,0,0) "marker/$index/position"
    }
    return @{
        name=$Name; verse_id=$verseId; fractal_id=$fractalId; petal_a_id=$petals[0]; petal_b_id=$petals[1]
        asset_id=$assetId; node_ids=$nodeIds; marker_node_ids=$markerIds; terrain_bound=$true
        terrain_petal_id=$boundIds[0]; terrain_petal_ids=@($boundIds)
        terrain_descriptor='terrain-fixture/descriptor.json'; terrain_tileset_id=$TerrainDescriptor.tileset_id
        query_rows_sql=$rowsSql; query_sum_sql=$sumSql; query_count=3; query_sum=60
        terrain_oracle=$TerrainDescriptor.oracle
    }
}
